#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! `content_call` uses the same request-then-flush framing as the other ABCI calls.

use std::net::TcpListener;
use std::thread;

use eld_tendermint_abci::{SocketClient, read_message, write_message};
use eld_tendermint_proto::abci::{
    Request, RequestFlush, Response, ResponseContent, ResponseFlush, request, response,
};
use eld_tendermint_proto::content::{self, ContentPush, ContentPushAck, Message};

#[test]
fn content_call_round_trips_a_receipt() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let request: Request = read_message(&mut sock).expect("request");
        let flush: Request = read_message(&mut sock).expect("flush");
        assert!(matches!(
            flush.value,
            Some(request::Value::Flush(RequestFlush {}))
        ));
        let Some(request::Value::Content(call)) = request.value else {
            panic!("not a content request: {request:?}");
        };
        assert_eq!(call.peer_id, "peer-1");
        let push = call.message.expect("message").sum.expect("body");
        let content::message::Sum::ContentPush(push) = push else {
            panic!("not a push");
        };
        assert_eq!(push.blob, b"hello");
        write_message(
            &mut sock,
            &Response {
                value: Some(response::Value::Content(ResponseContent {
                    message: Some(Message {
                        sum: Some(content::message::Sum::ContentPushAck(ContentPushAck {
                            request_id: push.request_id,
                            content_id: push.content_id,
                            provider: vec![1; 20],
                        })),
                    }),
                })),
            },
        )
        .expect("response");
        write_message(
            &mut sock,
            &Response {
                value: Some(response::Value::Flush(ResponseFlush {})),
            },
        )
        .expect("flush response");
    });

    let mut client = SocketClient::connect(addr).expect("connect");
    let reply = client
        .content_call(
            "peer-1",
            Message {
                sum: Some(content::message::Sum::ContentPush(ContentPush {
                    request_id: 7,
                    content_id: "id".to_owned(),
                    blob: b"hello".to_vec(),
                })),
            },
        )
        .expect("call");
    let ack = reply.message.expect("receipt").sum.expect("body");
    let content::message::Sum::ContentPushAck(ack) = ack else {
        panic!("not a receipt");
    };
    assert_eq!(ack.request_id, 7);
    assert_eq!(ack.provider, vec![1; 20]);
    server.join().expect("server");
}
