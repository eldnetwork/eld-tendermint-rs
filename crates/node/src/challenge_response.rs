//! Check that a capacity challenge response was signed by the challenged node.
//!
//! The signed bytes match `eld_common::capacity_proof::capacity_challenge_response_signing_bytes`:
//! JSON of the challenge id, the `0x` addresses, the chunk proofs, the time, and the
//! provider public key. This does not check that the chunk proofs are true.

use eld_tendermint_crypto::{PubKey, address_hash};
use eld_tendermint_proto::content::{self, CapacityChallengeResponse};
use serde::Serialize;

/// Whether `response` carries a signature from the challenged provider.
///
/// # Errors
///
/// Returns the reason the signature was not accepted.
pub fn verify_provider_signature(response: &CapacityChallengeResponse) -> Result<(), String> {
    let provider = address_bytes(&response.provider, "provider")?;
    let challenger = address_bytes(&response.challenger, "challenger")?;
    let pubkey = fixed_32(&response.provider_pubkey, "provider public key")?;
    let signature = fixed_64(&response.provider_signature, "provider signature")?;
    let derived = address_hash(&pubkey);
    if derived != provider {
        return Err("provider address does not match the provider public key".to_owned());
    }
    let proofs = proofs_from_proto(&response.proofs)?;
    let provider_pubkey = hex::encode(pubkey);
    let signing_bytes = signing_bytes(
        &response.challenge_id,
        &prefixed_hex(&provider),
        &prefixed_hex(&challenger),
        response.block_height,
        &proofs,
        response.generated_at,
        &provider_pubkey,
    )?;
    PubKey::from_bytes(&pubkey)
        .and_then(|key| key.verify(&signing_bytes, &signature))
        .map_err(|_| "provider signature was not valid".to_owned())
}

fn address_bytes(bytes: &[u8], field: &str) -> Result<[u8; 20], String> {
    bytes
        .try_into()
        .map_err(|_| format!("{field} must be 20 bytes"))
}

fn fixed_32(bytes: &[u8], field: &str) -> Result<[u8; 32], String> {
    bytes
        .try_into()
        .map_err(|_| format!("{field} must be 32 bytes"))
}

fn fixed_64(bytes: &[u8], field: &str) -> Result<[u8; 64], String> {
    bytes
        .try_into()
        .map_err(|_| format!("{field} must be 64 bytes"))
}

fn prefixed_hex(address: &[u8; 20]) -> String {
    format!("0x{}", hex::encode(address))
}

fn proofs_from_proto(proofs: &[content::ChunkProof]) -> Result<Vec<SignedChunkProof>, String> {
    proofs.iter().map(proof_from_proto).collect()
}

fn proof_from_proto(proof: &content::ChunkProof) -> Result<SignedChunkProof, String> {
    let chunk_index =
        usize::try_from(proof.chunk_index).map_err(|_| "chunk index does not fit".to_owned())?;
    let chunk_hash = fixed_32(&proof.chunk_hash, "chunk hash")?;
    let mut merkle_proof = Vec::with_capacity(proof.merkle_proof.len());
    for step in &proof.merkle_proof {
        merkle_proof.push(fixed_32(step, "merkle proof step")?);
    }
    let slot_state = slot_from_proto(proof.slot_state.as_ref().ok_or("chunk proof has no slot")?)?;
    Ok(SignedChunkProof {
        chunk_index,
        chunk_data: proof.chunk_data.clone(),
        chunk_hash,
        merkle_proof,
        slot_state,
    })
}

fn slot_from_proto(state: &content::SlotState) -> Result<SignedSlotState, String> {
    match state.kind.as_ref() {
        Some(content::slot_state::Kind::Proof(_)) => Ok(SignedSlotState::Proof),
        Some(content::slot_state::Kind::Open(_)) => Ok(SignedSlotState::Open),
        Some(content::slot_state::Kind::Content(slot)) => Ok(SignedSlotState::Content {
            deal_id: slot.deal_id.clone(),
            committed_hash: fixed_32(&slot.committed_hash, "committed hash")?,
        }),
        None => Err("chunk proof slot is empty".to_owned()),
    }
}

#[derive(Serialize)]
struct SigningPayload<'a> {
    challenge_id: &'a str,
    provider_id: &'a str,
    challenger: &'a str,
    block_height: i64,
    proofs: &'a [SignedChunkProof],
    generated_at: u64,
    provider_pubkey: &'a str,
}

#[derive(Serialize)]
struct SignedChunkProof {
    chunk_index: usize,
    chunk_data: Vec<u8>,
    chunk_hash: [u8; 32],
    merkle_proof: Vec<[u8; 32]>,
    slot_state: SignedSlotState,
}

#[derive(Serialize)]
enum SignedSlotState {
    Proof,
    Open,
    Content {
        deal_id: String,
        committed_hash: [u8; 32],
    },
}

fn signing_bytes(
    challenge_id: &str,
    provider_id: &str,
    challenger: &str,
    block_height: i64,
    proofs: &[SignedChunkProof],
    generated_at: u64,
    provider_pubkey: &str,
) -> Result<Vec<u8>, String> {
    let payload = SigningPayload {
        challenge_id,
        provider_id,
        challenger,
        block_height,
        proofs,
        generated_at,
        provider_pubkey,
    };
    serde_json::to_vec(&payload).map_err(|err| format!("signing payload was not encoded: {err}"))
}

#[cfg(test)]
mod tests {
    use super::verify_provider_signature;
    use eld_tendermint_crypto::{PrivKey, address_hash};
    use eld_tendermint_proto::content::{self, CapacityChallengeResponse};

    /// Go layout: 32-byte seed, then the verifying key of that seed.
    fn key_with_seed(seed: [u8; 32], pubkey_hex: &str) -> PrivKey {
        let pubkey = hex::decode(pubkey_hex).expect("public key hex");
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(&seed);
        bytes[32..].copy_from_slice(&pubkey);
        PrivKey::from_bytes(&bytes).expect("private key")
    }

    fn signed_response() -> CapacityChallengeResponse {
        let key = key_with_seed(
            [5u8; 32],
            "6e7a1cdd29b0b78fd13af4c5598feff4ef2a97166e3ca6f2e4fbfccd80505bf1",
        );
        let public_key = key.public_key().expect("public key");
        let provider = address_hash(public_key.as_bytes());
        let challenger = [0x44u8; 20];
        let proof = content::ChunkProof {
            chunk_index: 0,
            chunk_data: vec![1, 2, 3],
            chunk_hash: vec![9u8; 32],
            merkle_proof: vec![vec![8u8; 32]],
            slot_state: Some(content::SlotState {
                kind: Some(content::slot_state::Kind::Proof(content::ProofSlot {})),
            }),
        };
        let unsigned = CapacityChallengeResponse {
            challenge_id: "challenge-1".to_owned(),
            provider: provider.to_vec(),
            challenger: challenger.to_vec(),
            block_height: 42,
            proofs: vec![proof],
            generated_at: 1_710_000_000,
            provider_pubkey: public_key.as_bytes().to_vec(),
            provider_signature: Vec::new(),
        };
        let proofs = super::proofs_from_proto(&unsigned.proofs).expect("proofs");
        let message = super::signing_bytes(
            &unsigned.challenge_id,
            &super::prefixed_hex(&provider),
            &super::prefixed_hex(&challenger),
            unsigned.block_height,
            &proofs,
            unsigned.generated_at,
            &hex::encode(public_key.as_bytes()),
        )
        .expect("bytes");
        let signature = key.sign(&message).expect("sign");
        assert_eq!(
            String::from_utf8(message).expect("json"),
            r#"{"challenge_id":"challenge-1","provider_id":"0x7599776c3085e3f9da0d13071eb0b4ab50fd2bf6","challenger":"0x4444444444444444444444444444444444444444","block_height":42,"proofs":[{"chunk_index":0,"chunk_data":[1,2,3],"chunk_hash":[9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9],"merkle_proof":[[8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8]],"slot_state":"Proof"}],"generated_at":1710000000,"provider_pubkey":"6e7a1cdd29b0b78fd13af4c5598feff4ef2a97166e3ca6f2e4fbfccd80505bf1"}"#
        );
        CapacityChallengeResponse {
            provider_signature: signature.to_vec(),
            ..unsigned
        }
    }

    #[test]
    fn accepts_a_signature_from_the_challenged_node() {
        verify_provider_signature(&signed_response()).expect("valid signature");
    }

    #[test]
    fn rejects_a_changed_chunk() {
        let mut response = signed_response();
        response.proofs[0].chunk_data.push(99);
        let err = verify_provider_signature(&response).expect_err("tampered");
        assert!(err.contains("not valid"), "{err}");
    }

    #[test]
    fn rejects_a_public_key_for_a_different_provider() {
        let mut response = signed_response();
        response.provider_pubkey = vec![9u8; 32];
        let err = verify_provider_signature(&response).expect_err("wrong key");
        assert!(err.contains("does not match"), "{err}");
    }
}
