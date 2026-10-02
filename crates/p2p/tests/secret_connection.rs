//! Golden `deriveSecrets` vectors and an in-process handshake.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

use eld_tendermint_crypto::PrivKey;
use eld_tendermint_p2p::{derive_secrets, make_secret_connection};

#[test]
fn derive_secrets_matches_go_golden() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/TestDeriveSecretsAndChallengeGolden.golden"
    );
    let text = std::fs::read_to_string(path).expect("golden file");
    let mut lines = 0;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let params: Vec<&str> = line.split(',').collect();
        let secret = hex::decode(params[0]).expect("dh hex");
        let dh_secret: [u8; 32] = secret.try_into().expect("32 byte secret");
        let loc_is_least: bool = params[1].parse().expect("bool");
        let expected_recv = hex::decode(params[2]).expect("recv hex");
        let expected_send = hex::decode(params[3]).expect("send hex");

        let (recv, send) = derive_secrets(&dh_secret, loc_is_least);
        assert_eq!(recv.as_slice(), expected_recv.as_slice(), "recv");
        assert_eq!(send.as_slice(), expected_send.as_slice(), "send");
        lines += 1;
    }
    assert_eq!(lines, 32);
}

#[test]
fn peers_exchange_one_frame() {
    let key_a = PrivKey::generate();
    let key_b = PrivKey::generate();
    let pub_a = key_a.public_key().expect("public key");
    let pub_b = key_b.public_key().expect("public key");
    let (sock_a, sock_b) = UnixStream::pair().expect("socket pair");
    sock_a
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    sock_b
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");

    let handle = thread::spawn(move || {
        let mut conn = make_secret_connection(sock_b, &key_b).expect("handshake");
        assert_eq!(conn.remote_pub_key(), &pub_a);
        let mut buf = [0u8; 16];
        let n = conn.read(&mut buf).expect("read");
        assert_eq!(&buf[..n], b"ping");
    });

    let mut conn = make_secret_connection(sock_a, &key_a).expect("handshake");
    assert_eq!(conn.remote_pub_key(), &pub_b);
    conn.write_all(b"ping").expect("write");
    handle.join().expect("peer");
}
