//! `POST /` JSON-RPC for `status`, `health`, broadcast tx, block reads, and the tx index.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use eld_tendermint_crypto::sum;
use eld_tendermint_mempool::Mempool;
use eld_tendermint_proto::abci::{
    Event, EventAttribute, RequestQuery, ResponseCheckTx, ResponseDeliverTx, ResponseQuery,
    TxResult,
};
use eld_tendermint_proto::crypto::{ProofOp, ProofOps};
use eld_tendermint_state::TxIndex;
use eld_tendermint_store::{BlockStore, RocksDb};
use eld_tendermint_types::{Block, BlockId, Commit, CommitSig, Header, Time, Tx};
use serde_json::{Map, Value};

use crate::app::AbciApp;
use crate::error::{Error, fail};
use crate::wait::TxWaiter;

/// Fields `status` reads. Height comes from the RocksDB block store.
pub struct NodeStatus {
    pub id: String,
    pub network: String,
    pub listen_addr: String,
    pub moniker: String,
    pub rpc_address: String,
    pub channels: String,
    pub address: String,
    pub pub_key: Value,
    pub voting_power: i64,
    pub block_store: Arc<BlockStore<RocksDb>>,
    pub mempool: Arc<Mutex<Mempool<AbciApp>>>,
    pub waiter: Arc<TxWaiter>,
    pub commit_timeout: Duration,
    pub app: AbciApp,
    /// Open when `[tx_index] indexer` is not `null`.
    pub tx_index: Option<Arc<TxIndex<RocksDb>>>,
}

/// Accept JSON-RPC calls until the listener closes.
///
/// # Errors
///
/// Returns an error when a response cannot be encoded. A bad client does not
/// stop the loop.
pub fn serve(listener: TcpListener, status: Arc<NodeStatus>) -> Result<(), Error> {
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let _ = handle(&mut stream, &status);
    }
    Ok(())
}

fn handle(stream: &mut impl ReadWrite, status: &NodeStatus) -> Result<(), Error> {
    let (request_line, _headers, body) = read_request(stream)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if method != "POST" || path != "/" {
        write_raw(stream, 404, "Not Found", b"")?;
        return Ok(());
    }
    let response = match serde_json::from_slice::<Value>(&body) {
        Ok(request) => dispatch(&request, status),
        Err(_) => rpc_error(Value::Null, -32700, "Parse error. Invalid JSON"),
    };
    let bytes = serde_json::to_vec(&response).map_err(fail)?;
    write_raw(stream, 200, "OK", &bytes)
}

fn dispatch(request: &Value, status: &NodeStatus) -> Value {
    let id = echo_id(request);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "health" => rpc_result(id, Value::Object(Map::new())),
        "status" => rpc_result(id, status_result(status)),
        "broadcast_tx_sync" => broadcast_tx_sync(&id, request, status),
        "broadcast_tx_commit" => broadcast_tx_commit(&id, request, status),
        "abci_query" => abci_query(&id, request, status),
        "block" => rpc_block(&id, request, status),
        "commit" => rpc_commit(&id, request, status),
        "tx" => rpc_tx(&id, request, status),
        "tx_search" => rpc_tx_search(&id, request, status),
        _ => rpc_error(id.clone(), -32601, "Method not found"),
    }
}

fn status_result(status: &NodeStatus) -> Value {
    let height = status.block_store.height();
    let (latest_hash, latest_app, latest_time) = block_at(&status.block_store, height);
    let earliest_height = if height == 0 {
        0
    } else {
        status.block_store.base()
    };
    let (earliest_hash, earliest_app, earliest_time) =
        block_at(&status.block_store, earliest_height);
    serde_json::json!({
        "node_info": {
            "protocol_version": { "p2p": 8, "block": 11, "app": 0 },
            "id": status.id,
            "listen_addr": status.listen_addr,
            "network": status.network,
            "version": "0.34.24",
            "channels": status.channels,
            "moniker": status.moniker,
            "other": {
                "tx_index": if status.tx_index.is_some() { "on" } else { "off" },
                "rpc_address": status.rpc_address,
            },
        },
        "sync_info": {
            "latest_block_hash": latest_hash,
            "latest_app_hash": latest_app,
            "latest_block_height": height,
            "latest_block_time": latest_time,
            "earliest_block_hash": earliest_hash,
            "earliest_app_hash": earliest_app,
            "earliest_block_height": earliest_height,
            "earliest_block_time": earliest_time,
            "catching_up": false,
        },
        "validator_info": {
            "address": status.address,
            "pub_key": status.pub_key,
            "voting_power": status.voting_power,
        },
    })
}

fn block_at(store: &BlockStore<RocksDb>, height: i64) -> (String, String, String) {
    let epoch = Time::from_unix_parts(0, 0).to_rfc3339();
    if height == 0 {
        return (String::new(), String::new(), epoch);
    }
    let Some(meta) = store.load_block_meta(height) else {
        return (String::new(), String::new(), epoch);
    };
    (
        hex_upper(&meta.block_id.hash),
        hex_upper(&meta.header.app_hash),
        meta.header.time.to_rfc3339(),
    )
}

fn hex_upper(bytes: &[u8]) -> String {
    hex::encode(bytes).to_ascii_uppercase()
}

fn broadcast_tx_sync(id: &Value, request: &Value, status: &NodeStatus) -> Value {
    let tx = match tx_param(request) {
        Ok(tx) => tx,
        Err(data) => return rpc_error_data(id.clone(), -32602, "Invalid params", &data),
    };
    match check_tx(status, &tx) {
        Ok(response) => rpc_result(id.clone(), sync_result(&response, &tx)),
        Err(err) => internal_error(id.clone(), &err),
    }
}

fn broadcast_tx_commit(id: &Value, request: &Value, status: &NodeStatus) -> Value {
    let tx = match tx_param(request) {
        Ok(tx) => tx,
        Err(data) => return rpc_error_data(id.clone(), -32602, "Invalid params", &data),
    };
    let hash = sum(&tx);
    let subscription = status.waiter.subscribe(hash);
    let response = match check_tx(status, &tx) {
        Ok(response) => response,
        Err(err) => {
            status.waiter.cancel(subscription);
            return internal_error(id.clone(), &err);
        }
    };
    if response.code != 0 {
        status.waiter.cancel(subscription);
        return rpc_result(
            id.clone(),
            commit_result(&response, &ResponseDeliverTx::default(), &hash, 0),
        );
    }
    match subscription.recv_timeout(status.commit_timeout) {
        Ok(delivered) => rpc_result(
            id.clone(),
            commit_result(&response, &delivered.response, &hash, delivered.height),
        ),
        Err(_) => {
            status.waiter.cancel(subscription);
            rpc_error_data(
                id.clone(),
                -32603,
                "Internal error",
                "timed out waiting for tx to be included in a block",
            )
        }
    }
}

fn check_tx(
    status: &NodeStatus,
    tx: &[u8],
) -> Result<ResponseCheckTx, eld_tendermint_mempool::Error> {
    let mut pool = status.mempool.lock().unwrap_or_else(|err| err.into_inner());
    pool.check_tx_response(&Tx::new(tx.to_vec()))
}

/// `params.tx` is standard base64, the amino JSON encoding of `[]byte`.
fn tx_param(request: &Value) -> Result<Vec<u8>, String> {
    let Some(text) = request
        .get("params")
        .and_then(|params| params.get("tx"))
        .and_then(Value::as_str)
    else {
        return Err("missing params.tx".to_owned());
    };
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| "params.tx is not base64".to_owned())
}

fn sync_result(response: &ResponseCheckTx, tx: &[u8]) -> Value {
    serde_json::json!({
        "code": response.code,
        "data": hex_upper(&response.data),
        "log": response.log,
        "codespace": response.codespace,
        "hash": hex_upper(&sum(tx)),
    })
}

fn commit_result(
    check_tx: &ResponseCheckTx,
    deliver_tx: &ResponseDeliverTx,
    hash: &[u8; 32],
    height: i64,
) -> Value {
    serde_json::json!({
        "check_tx": check_tx_json(check_tx),
        "deliver_tx": deliver_tx_json(deliver_tx),
        "hash": hex_upper(hash),
        "height": height,
    })
}

fn check_tx_json(response: &ResponseCheckTx) -> Value {
    serde_json::json!({
        "code": response.code,
        "data": b64(&response.data),
        "log": response.log,
        "info": response.info,
        "gas_wanted": response.gas_wanted.to_string(),
        "gas_used": response.gas_used.to_string(),
        "events": events_json(&response.events),
        "codespace": response.codespace,
        "sender": response.sender,
        "priority": response.priority.to_string(),
        "mempoolError": response.mempool_error,
    })
}

fn deliver_tx_json(response: &ResponseDeliverTx) -> Value {
    serde_json::json!({
        "code": response.code,
        "data": b64(&response.data),
        "log": response.log,
        "info": response.info,
        "gas_wanted": response.gas_wanted.to_string(),
        "gas_used": response.gas_used.to_string(),
        "events": events_json(&response.events),
        "codespace": response.codespace,
    })
}

fn events_json(events: &[Event]) -> Value {
    Value::Array(events.iter().map(event_json).collect())
}

fn event_json(event: &Event) -> Value {
    serde_json::json!({
        "type": event.r#type,
        "attributes": event.attributes.iter().map(attribute_json).collect::<Vec<_>>(),
    })
}

fn attribute_json(attribute: &EventAttribute) -> Value {
    serde_json::json!({
        "key": b64(&attribute.key),
        "value": b64(&attribute.value),
        "index": attribute.index,
    })
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn abci_query(id: &Value, request: &Value, status: &NodeStatus) -> Value {
    let params = match params_object(request) {
        Ok(params) => params,
        Err(data) => return invalid_params(id, &data),
    };
    let path = match string_param(params, "path") {
        Ok(path) => path,
        Err(data) => return invalid_params(id, &data),
    };
    let data = match hex_param(params, "data") {
        Ok(data) => data,
        Err(data) => return invalid_params(id, &data),
    };
    let height = match i64_param(params, "height") {
        Ok(height) => height.unwrap_or(0),
        Err(data) => return invalid_params(id, &data),
    };
    let prove = match bool_param(params, "prove") {
        Ok(prove) => prove,
        Err(data) => return invalid_params(id, &data),
    };
    match status.app.query(RequestQuery {
        data: data.into(),
        path,
        height,
        prove,
    }) {
        Ok(response) => rpc_result(
            id.clone(),
            serde_json::json!({ "response": query_json(&response) }),
        ),
        Err(err) => internal_error(id.clone(), &err),
    }
}

fn query_json(response: &ResponseQuery) -> Value {
    serde_json::json!({
        "code": response.code,
        "log": response.log,
        "info": response.info,
        "index": response.index.to_string(),
        "key": b64(&response.key),
        "value": b64(&response.value),
        "proofOps": proof_ops_json(response.proof_ops.as_ref()),
        "height": response.height.to_string(),
        "codespace": response.codespace,
    })
}

fn proof_ops_json(proof: Option<&ProofOps>) -> Value {
    match proof {
        None => Value::Null,
        Some(proof) => serde_json::json!({
            "ops": proof.ops.iter().map(proof_op_json).collect::<Vec<_>>(),
        }),
    }
}

fn proof_op_json(op: &ProofOp) -> Value {
    serde_json::json!({
        "type": op.r#type,
        "key": b64(&op.key),
        "data": b64(&op.data),
    })
}

fn rpc_block(id: &Value, request: &Value, status: &NodeStatus) -> Value {
    let height = match store_height(id, request, status) {
        Ok(height) => height,
        Err(error) => return error,
    };
    let Some(meta) = status.block_store.load_block_meta(height) else {
        return missing_block(id, height);
    };
    let Some(block) = status.block_store.load_block(height) else {
        return missing_block(id, height);
    };
    rpc_result(
        id.clone(),
        serde_json::json!({
            "block_id": block_id_json(&meta.block_id),
            "block": block_json(&block),
        }),
    )
}

fn rpc_commit(id: &Value, request: &Value, status: &NodeStatus) -> Value {
    let height = match store_height(id, request, status) {
        Ok(height) => height,
        Err(error) => return error,
    };
    let Some(meta) = status.block_store.load_block_meta(height) else {
        return missing_block(id, height);
    };
    let latest = status.block_store.height();
    let commit = if height == latest {
        status.block_store.load_seen_commit(height)
    } else {
        status.block_store.load_block_commit(height)
    };
    let Some(commit) = commit else {
        return internal_message(id, &format!("commit {height} not found"));
    };
    rpc_result(
        id.clone(),
        serde_json::json!({
            "signed_header": {
                "header": header_json(&meta.header),
                "commit": commit_json(&commit),
            },
            "canonical": height != latest,
        }),
    )
}

/// Missing height and `0` mean the latest block. A bad type is invalid params.
/// A height outside the store is an internal error with Go's text.
fn store_height(id: &Value, request: &Value, status: &NodeStatus) -> Result<i64, Value> {
    let params = match params_object(request) {
        Ok(params) => params,
        Err(data) => return Err(invalid_params(id, &data)),
    };
    let asked = match i64_param(params, "height") {
        Ok(height) => height,
        Err(data) => return Err(invalid_params(id, &data)),
    };
    let latest = status.block_store.height();
    let height = match asked {
        None | Some(0) => latest,
        Some(height) if height < 0 => {
            return Err(internal_message(
                id,
                &format!("height must be greater than 0, but got {height}"),
            ));
        }
        Some(height) => height,
    };
    if height <= 0 {
        return Err(missing_block(id, height));
    }
    if height > latest {
        return Err(internal_message(
            id,
            &format!(
                "height {height} must be less than or equal to the current blockchain height {latest}"
            ),
        ));
    }
    let base = status.block_store.base();
    if height < base {
        return Err(internal_message(
            id,
            &format!("height {height} is not available, lowest height is {base}"),
        ));
    }
    Ok(height)
}

fn missing_block(id: &Value, height: i64) -> Value {
    internal_message(id, &format!("block {height} not found"))
}

fn block_json(block: &Block) -> Value {
    serde_json::json!({
        "header": header_json(&block.header),
        "data": {
            "txs": block.data.as_slice().iter().map(|tx| b64(tx.as_bytes())).collect::<Vec<_>>(),
        },
        "evidence": { "evidence": [] },
        "last_commit": block.last_commit.as_ref().map(commit_json),
    })
}

fn header_json(header: &Header) -> Value {
    serde_json::json!({
        "version": {
            "block": header.version.block.to_string(),
            "app": header.version.app.to_string(),
        },
        "chain_id": header.chain_id.as_str(),
        "height": header.height.to_string(),
        "time": header.time.to_rfc3339(),
        "last_block_id": block_id_json(&header.last_block_id),
        "last_commit_hash": hex_upper(&header.last_commit_hash),
        "data_hash": hex_upper(&header.data_hash),
        "validators_hash": hex_upper(&header.validators_hash),
        "next_validators_hash": hex_upper(&header.next_validators_hash),
        "consensus_hash": hex_upper(&header.consensus_hash),
        "app_hash": hex_upper(&header.app_hash),
        "last_results_hash": hex_upper(&header.last_results_hash),
        "evidence_hash": hex_upper(&header.evidence_hash),
        "proposer_address": hex_upper(&header.proposer_address),
    })
}

fn block_id_json(block_id: &BlockId) -> Value {
    serde_json::json!({
        "hash": hex_upper(&block_id.hash),
        "parts": {
            "total": block_id.part_set_header.total,
            "hash": hex_upper(&block_id.part_set_header.hash),
        },
    })
}

fn commit_json(commit: &Commit) -> Value {
    serde_json::json!({
        "height": commit.height.to_string(),
        "round": commit.round,
        "block_id": block_id_json(&commit.block_id),
        "signatures": commit.signatures.iter().map(commit_sig_json).collect::<Vec<_>>(),
    })
}

fn commit_sig_json(sig: &CommitSig) -> Value {
    serde_json::json!({
        "block_id_flag": sig.block_id_flag as i32,
        "validator_address": hex_upper(&sig.validator_address),
        "timestamp": sig.timestamp.to_rfc3339(),
        "signature": if sig.signature.is_empty() {
            Value::Null
        } else {
            Value::String(b64(&sig.signature))
        },
    })
}

const MAX_QUERY_LENGTH: usize = 512;
const DEFAULT_PER_PAGE: i64 = 30;
const MAX_PER_PAGE: i64 = 100;

fn rpc_tx(id: &Value, request: &Value, status: &NodeStatus) -> Value {
    let Some(index) = &status.tx_index else {
        return internal_message(id, "transaction indexing is disabled");
    };
    let params = match params_object(request) {
        Ok(params) => params,
        Err(data) => return invalid_params(id, &data),
    };
    if let Err(data) = bool_param(params, "prove") {
        return invalid_params(id, &data);
    }
    let hash = match hash_param(params) {
        Ok(hash) => hash,
        Err(HashParam::Empty) => {
            return internal_message(id, "transaction hash cannot be empty");
        }
        Err(HashParam::Invalid(data)) => return invalid_params(id, &data),
    };
    match index.get(&hash) {
        Ok(Some(tx)) => rpc_result(id.clone(), result_tx_json(&hash, &tx)),
        Ok(None) => internal_message(id, &format!("tx ({}) not found", hex_upper(&hash))),
        Err(err) => internal_error(id.clone(), &err),
    }
}

fn rpc_tx_search(id: &Value, request: &Value, status: &NodeStatus) -> Value {
    let Some(index) = &status.tx_index else {
        return internal_message(id, "transaction indexing is disabled");
    };
    let params = match params_object(request) {
        Ok(params) => params,
        Err(data) => return invalid_params(id, &data),
    };
    if let Err(data) = bool_param(params, "prove") {
        return invalid_params(id, &data);
    }
    let query = match string_param(params, "query") {
        Ok(query) => query,
        Err(data) => return invalid_params(id, &data),
    };
    let page = match i64_param(params, "page") {
        Ok(page) => page,
        Err(data) => return invalid_params(id, &data),
    };
    let per_page = match i64_param(params, "per_page") {
        Ok(per_page) => per_page,
        Err(data) => return invalid_params(id, &data),
    };
    let order_by = match string_param(params, "order_by") {
        Ok(order_by) => order_by,
        Err(data) => return invalid_params(id, &data),
    };
    let mut txs = match search_query(index, &query) {
        Ok(txs) => txs,
        Err(data) => return internal_message(id, &data),
    };
    let desc = match order_by.as_str() {
        "desc" => true,
        "asc" | "" => false,
        _ => {
            return internal_message(
                id,
                "expected order_by to be either `asc` or `desc` or empty",
            );
        }
    };
    txs.sort_by(|left, right| {
        let order = left
            .height
            .cmp(&right.height)
            .then(left.index.cmp(&right.index));
        if desc { order.reverse() } else { order }
    });
    let total = i64::try_from(txs.len()).unwrap_or(i64::MAX);
    let per_page = validate_per_page(per_page);
    let page = match validate_page(page, per_page, total) {
        Ok(page) => page,
        Err(data) => return internal_message(id, &data),
    };
    let skip = (page - 1) * per_page;
    let end = (skip + per_page).min(total);
    let page_txs: Vec<Value> = txs[skip as usize..end as usize]
        .iter()
        .map(|tx| result_tx_json(&sum(tx.tx.as_ref()), tx))
        .collect();
    rpc_result(
        id.clone(),
        serde_json::json!({
            "txs": page_txs,
            "total_count": total,
        }),
    )
}

fn result_tx_json(hash: &[u8], tx: &TxResult) -> Value {
    let deliver = tx.result.clone().unwrap_or_default();
    serde_json::json!({
        "hash": hex_upper(hash),
        "height": tx.height,
        "index": tx.index,
        "tx_result": deliver_tx_json(&deliver),
        "tx": b64(tx.tx.as_ref()),
    })
}

enum HashParam {
    Empty,
    Invalid(String),
}

/// Hex, including a `0x` prefix, or standard base64. Even-length hex wins over base64.
fn hash_param(params: &Value) -> Result<Vec<u8>, HashParam> {
    let Some(value) = field(params, "hash") else {
        return Err(HashParam::Empty);
    };
    let Value::String(text) = value else {
        return Err(HashParam::Invalid("params.hash is not a string".to_owned()));
    };
    if text.is_empty() {
        return Err(HashParam::Empty);
    }
    let decoded = if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        hex::decode(rest).map_err(|_| HashParam::Invalid("params.hash is not hex".to_owned()))?
    } else if text.len() % 2 == 0 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        hex::decode(text).map_err(|_| HashParam::Invalid("params.hash is not hex".to_owned()))?
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|_| HashParam::Invalid("params.hash is not hex or base64".to_owned()))?
    };
    if decoded.is_empty() {
        return Err(HashParam::Empty);
    }
    Ok(decoded)
}

enum TxSearch {
    Hash(Vec<u8>),
    Height(i64),
    None,
}

fn search_query(index: &TxIndex<RocksDb>, query: &str) -> Result<Vec<TxResult>, String> {
    if query.len() > MAX_QUERY_LENGTH {
        return Err("maximum query length exceeded".to_owned());
    }
    match parse_tx_query(query)? {
        TxSearch::Hash(hash) => index
            .get(&hash)
            .map(|found| found.into_iter().collect())
            .map_err(|err| err.to_string()),
        TxSearch::Height(height) => index.by_height(height).map_err(|err| err.to_string()),
        TxSearch::None => Ok(Vec::new()),
    }
}

/// Equality on `tx.height` and `tx.hash` only. A hash condition wins, as in `lookForHash`.
fn parse_tx_query(query: &str) -> Result<TxSearch, String> {
    let parts: Vec<&str> = if query.contains(" AND ") {
        query.split(" AND ").collect()
    } else {
        vec![query]
    };
    let mut hash = None;
    let mut height = None;
    let mut heights_disagree = false;
    for part in parts {
        let part = part.trim();
        let (tag, rest) = split_condition(part)?;
        if tag != "tx.height" && tag != "tx.hash" {
            return Err(format!("tag {tag} is not supported"));
        }
        let rest = rest.trim();
        let Some(operand) = rest.strip_prefix('=') else {
            return Err(format!("tag {tag} is not supported"));
        };
        let operand = operand.trim();
        if tag == "tx.hash" {
            let Some(quoted) = operand
                .strip_prefix('\'')
                .and_then(|text| text.strip_suffix('\''))
            else {
                return Err("tag tx.hash is not supported".to_owned());
            };
            if hash.is_none() {
                hash = Some(decode_query_hash(quoted)?);
            }
        } else {
            let Ok(parsed) = operand.parse::<i64>() else {
                return Err("tag tx.height is not supported".to_owned());
            };
            if let Some(seen) = height {
                if seen != parsed {
                    heights_disagree = true;
                }
            } else {
                height = Some(parsed);
            }
        }
    }
    if let Some(hash) = hash {
        return Ok(TxSearch::Hash(hash));
    }
    if heights_disagree {
        return Ok(TxSearch::None);
    }
    if let Some(height) = height {
        return Ok(TxSearch::Height(height));
    }
    Err("tag  is not supported".to_owned())
}

fn split_condition(part: &str) -> Result<(&str, &str), String> {
    let end = part
        .find([' ', '='])
        .filter(|end| *end > 0)
        .ok_or_else(|| "tag  is not supported".to_owned())?;
    Ok((&part[..end], &part[end..]))
}

fn decode_query_hash(text: &str) -> Result<Vec<u8>, String> {
    if text.len() % 2 != 0 {
        return Err(
            "error during searching for a hash in the query: encoding/hex: odd length hex string"
                .to_owned(),
        );
    }
    hex::decode(text)
        .map_err(|err| format!("error during searching for a hash in the query: {err}"))
}

fn validate_per_page(per_page: Option<i64>) -> i64 {
    match per_page {
        Some(value) if (1..=MAX_PER_PAGE).contains(&value) => value,
        Some(value) if value > MAX_PER_PAGE => MAX_PER_PAGE,
        _ => DEFAULT_PER_PAGE,
    }
}

fn validate_page(page: Option<i64>, per_page: i64, total: i64) -> Result<i64, String> {
    let Some(page) = page else {
        return Ok(1);
    };
    let mut pages = if total == 0 {
        1
    } else {
        (total - 1) / per_page + 1
    };
    if pages == 0 {
        pages = 1;
    }
    if page <= 0 || page > pages {
        return Err(format!(
            "page should be within [1, {pages}] range, given {page}"
        ));
    }
    Ok(page)
}

/// A missing `params` object uses field defaults. Anything else is invalid.
fn params_object(request: &Value) -> Result<&Value, String> {
    match request.get("params") {
        None | Some(Value::Null) => Ok(&Value::Null),
        Some(params) if params.is_object() => Ok(params),
        Some(_) => Err("params must be an object".to_owned()),
    }
}

fn field<'a>(params: &'a Value, name: &str) -> Option<&'a Value> {
    params.get(name).filter(|value| !value.is_null())
}

fn string_param(params: &Value, name: &str) -> Result<String, String> {
    match field(params, name) {
        None => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(format!("params.{name} is not a string")),
    }
}

fn hex_param(params: &Value, name: &str) -> Result<Vec<u8>, String> {
    match field(params, name) {
        None => Ok(Vec::new()),
        Some(Value::String(text)) => {
            hex::decode(text).map_err(|_| format!("params.{name} is not hex"))
        }
        Some(_) => Err(format!("params.{name} is not hex")),
    }
}

fn bool_param(params: &Value, name: &str) -> Result<bool, String> {
    match field(params, name) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("params.{name} is not a bool")),
    }
}

/// `None` when the field is missing. `0` is a present zero, not missing.
fn i64_param(params: &Value, name: &str) -> Result<Option<i64>, String> {
    match field(params, name) {
        None => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("params.{name} is not an integer")),
        Some(Value::String(text)) => text
            .parse::<i64>()
            .map(Some)
            .map_err(|_| format!("params.{name} is not an integer")),
        Some(_) => Err(format!("params.{name} is not an integer")),
    }
}

fn invalid_params(id: &Value, data: &str) -> Value {
    rpc_error_data(id.clone(), -32602, "Invalid params", data)
}

fn internal_message(id: &Value, data: &str) -> Value {
    rpc_error_data(id.clone(), -32603, "Internal error", data)
}

fn internal_error(id: Value, err: &impl std::fmt::Display) -> Value {
    rpc_error_data(id, -32603, "Internal error", &err.to_string())
}

fn echo_id(request: &Value) -> Value {
    match request.get("id") {
        Some(Value::Number(number)) => Value::Number(number.clone()),
        Some(Value::String(text)) => Value::String(text.clone()),
        _ => Value::Null,
    }
}

fn rpc_result(id: Value, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn rpc_error_data(id: Value, code: i64, message: &str, data: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message, "data": data },
    })
}

fn read_request(stream: &mut impl Read) -> Result<(String, Vec<String>, Vec<u8>), Error> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).map_err(fail)?;
        if n == 0 {
            return Err(Error::new("client closed the request"));
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(Error::new("request headers are too large"));
        }
    }
    let text = String::from_utf8(buf).map_err(|_| Error::new("request headers are not utf-8"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        headers.push(line.to_owned());
    }
    if content_length > 1024 * 1024 {
        return Err(Error::new("request body is too large"));
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).map_err(fail)?;
    }
    Ok((request_line, headers, body))
}

fn write_raw(stream: &mut impl Write, status: u16, reason: &str, body: &[u8]) -> Result<(), Error> {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).map_err(fail)?;
    stream.write_all(body).map_err(fail)?;
    stream.flush().map_err(fail)
}

trait ReadWrite: Read + Write {}

impl<T: Read + Write> ReadWrite for T {}
