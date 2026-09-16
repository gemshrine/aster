mod common;

use std::time::Duration;

use common::{ALICE_TOKEN, BOB_TOKEN, Client, assert_welcome, spawn_server};
use protocol::{ErrorCode, PROTOCOL_VERSION, RejectReason, ServerMessage};

#[tokio::test]
async fn health_check_returns_ok() {
    let addr = spawn_server().await;
    let response = http_get(addr, "/health").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
}

/// Minimal HTTP GET to avoid pulling in an HTTP client for a single test.
async fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn valid_token_gets_welcome() {
    let addr = spawn_server().await;
    let (_alice, welcome) = Client::login(addr, ALICE_TOKEN).await;
    assert_welcome(&welcome, "alice", false);
}

#[tokio::test]
async fn invalid_token_is_rejected_and_closed() {
    let addr = spawn_server().await;
    let mut client = Client::connect(addr).await;
    client.hello(PROTOCOL_VERSION, "wrong-token").await;
    assert_eq!(
        client.recv().await,
        ServerMessage::Rejected {
            reason: RejectReason::InvalidToken
        }
    );
    client.expect_closed().await;
}

#[tokio::test]
async fn protocol_version_mismatch_is_rejected() {
    let addr = spawn_server().await;
    let mut client = Client::connect(addr).await;
    client.hello(PROTOCOL_VERSION + 1, ALICE_TOKEN).await;
    assert_eq!(
        client.recv().await,
        ServerMessage::Rejected {
            reason: RejectReason::UnsupportedProtocolVersion
        }
    );
    client.expect_closed().await;
}

#[tokio::test]
async fn messages_before_hello_are_refused() {
    let addr = spawn_server().await;
    let mut client = Client::connect(addr).await;

    client
        .send_raw(r#"{"type":"signal","payload":{"type":"call_end"}}"#)
        .await;
    assert!(matches!(
        client.recv().await,
        ServerMessage::Error {
            code: ErrorCode::NotAuthenticated,
            ..
        }
    ));

    client.send_raw("not json").await;
    assert!(matches!(
        client.recv().await,
        ServerMessage::Error {
            code: ErrorCode::MalformedMessage,
            ..
        }
    ));

    client.hello(PROTOCOL_VERSION, ALICE_TOKEN).await;
    assert!(matches!(client.recv().await, ServerMessage::Welcome { .. }));
}

#[tokio::test]
async fn presence_follows_peer_connect_and_disconnect() {
    let addr = spawn_server().await;
    let (mut alice, _) = Client::login(addr, ALICE_TOKEN).await;

    let (bob, welcome) = Client::login(addr, BOB_TOKEN).await;
    assert_welcome(&welcome, "bob", true);
    assert_eq!(
        alice.recv().await,
        ServerMessage::PeerStatus { online: true }
    );

    bob.close().await;
    assert_eq!(
        alice.recv().await,
        ServerMessage::PeerStatus { online: false }
    );
}

#[tokio::test]
async fn new_session_replaces_old_one() {
    let addr = spawn_server().await;
    let (mut alice_old, _) = Client::login(addr, ALICE_TOKEN).await;
    let (mut bob, _) = Client::login(addr, BOB_TOKEN).await;
    assert_eq!(
        alice_old.recv().await,
        ServerMessage::PeerStatus { online: true }
    );

    let (_alice_new, welcome) = Client::login(addr, ALICE_TOKEN).await;
    assert_welcome(&welcome, "alice", true);

    assert!(matches!(
        alice_old.recv().await,
        ServerMessage::Error {
            code: ErrorCode::SessionReplaced,
            ..
        }
    ));
    alice_old.expect_closed().await;

    // Alice never went offline from Bob's point of view.
    assert_eq!(bob.try_recv(Duration::from_millis(300)).await, None);
}

#[tokio::test]
async fn welcome_carries_ice_servers_with_turn_credentials() {
    let addr = spawn_server().await;
    let (_alice, welcome) = Client::login(addr, ALICE_TOKEN).await;
    let ServerMessage::Welcome { ice_servers, .. } = welcome else {
        unreachable!()
    };

    assert_eq!(ice_servers.len(), 2);
    assert_eq!(ice_servers[0].urls, ["stun:turn.test:3478"]);
    let relay = &ice_servers[1];
    assert!(
        relay
            .urls
            .iter()
            .all(|url| url.starts_with("turn:turn.test:3478"))
    );

    let username = relay.username.as_deref().unwrap();
    let (expiry, user) = username.split_once(':').unwrap();
    assert_eq!(user, "alice");
    let expiry: u64 = expiry.parse().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(expiry > now + 60 * 60, "credentials should outlive a call");
    assert!(relay.credential.as_deref().is_some_and(|c| !c.is_empty()));
}
