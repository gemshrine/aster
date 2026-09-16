mod common;

use std::time::Duration;

use common::{ALICE_TOKEN, BOB_TOKEN, Client, spawn_server, spawn_server_with_limits};
use protocol::{ClientMessage, ErrorCode, ServerMessage, SignalPayload, UserId};
use signaling_server::hub::QueueLimits;
use uuid::Uuid;

fn chat(body: &str) -> (Uuid, ClientMessage) {
    let id = Uuid::new_v4();
    (
        id,
        ClientMessage::ChatMessage {
            id,
            body: body.into(),
            sent_at: 1_700_000_000_000,
        },
    )
}

/// Logs in both users and drains the presence notification Alice gets.
async fn both_online(addr: std::net::SocketAddr) -> (Client, Client) {
    let (mut alice, _) = Client::login(addr, ALICE_TOKEN).await;
    let (bob, _) = Client::login(addr, BOB_TOKEN).await;
    assert_eq!(
        alice.recv().await,
        ServerMessage::PeerStatus { online: true }
    );
    (alice, bob)
}

#[tokio::test]
async fn signals_are_relayed_unchanged_both_ways() {
    let addr = spawn_server().await;
    let (mut alice, mut bob) = both_online(addr).await;

    let payloads = [
        SignalPayload::Offer {
            sdp: "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\n".into(),
        },
        SignalPayload::IceCandidate {
            candidate: "candidate:1 1 UDP 2122252543 10.0.0.1 5000 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        },
        SignalPayload::CallEnd,
    ];
    for payload in payloads.clone() {
        alice.send(&ClientMessage::Signal { payload }).await;
    }
    for payload in payloads {
        assert_eq!(bob.recv().await, ServerMessage::Signal { payload });
    }

    let answer = SignalPayload::Answer { sdp: "v=0".into() };
    bob.send(&ClientMessage::Signal {
        payload: answer.clone(),
    })
    .await;
    assert_eq!(
        alice.recv().await,
        ServerMessage::Signal { payload: answer }
    );
}

#[tokio::test]
async fn signal_to_offline_peer_is_an_error() {
    let addr = spawn_server().await;
    let (mut alice, _) = Client::login(addr, ALICE_TOKEN).await;
    alice
        .send(&ClientMessage::Signal {
            payload: SignalPayload::CallEnd,
        })
        .await;
    assert!(matches!(
        alice.recv().await,
        ServerMessage::Error {
            code: ErrorCode::PeerOffline,
            ..
        }
    ));
}

#[tokio::test]
async fn chat_to_online_peer_is_delivered() {
    let addr = spawn_server().await;
    let (mut alice, mut bob) = both_online(addr).await;

    let (id, msg) = chat("привет");
    alice.send(&msg).await;

    assert_eq!(
        bob.recv().await,
        ServerMessage::ChatMessage {
            id,
            from: UserId("alice".into()),
            body: "привет".into(),
            sent_at: 1_700_000_000_000,
        }
    );
    assert_eq!(alice.recv().await, ServerMessage::ChatDelivered { id });
}

#[tokio::test]
async fn chat_to_offline_peer_is_queued_and_delivered_on_connect() {
    let addr = spawn_server().await;
    let (mut alice, _) = Client::login(addr, ALICE_TOKEN).await;

    let (first, msg) = chat("one");
    alice.send(&msg).await;
    assert_eq!(alice.recv().await, ServerMessage::ChatQueued { id: first });
    let (second, msg) = chat("two");
    alice.send(&msg).await;
    assert_eq!(alice.recv().await, ServerMessage::ChatQueued { id: second });

    let (mut bob, welcome) = Client::login(addr, BOB_TOKEN).await;
    assert!(matches!(
        welcome,
        ServerMessage::Welcome {
            peer_online: true,
            ..
        }
    ));
    for (id, body) in [(first, "one"), (second, "two")] {
        assert_eq!(
            bob.recv().await,
            ServerMessage::ChatMessage {
                id,
                from: UserId("alice".into()),
                body: body.into(),
                sent_at: 1_700_000_000_000,
            }
        );
    }

    assert_eq!(
        alice.recv().await,
        ServerMessage::ChatDelivered { id: first }
    );
    assert_eq!(
        alice.recv().await,
        ServerMessage::ChatDelivered { id: second }
    );
    assert_eq!(
        alice.recv().await,
        ServerMessage::PeerStatus { online: true }
    );

    // The queue is drained: reconnecting does not redeliver.
    bob.close().await;
    assert_eq!(
        alice.recv().await,
        ServerMessage::PeerStatus { online: false }
    );
    let (mut bob, _) = Client::login(addr, BOB_TOKEN).await;
    assert_eq!(bob.try_recv(Duration::from_millis(300)).await, None);
}

#[tokio::test]
async fn full_offline_queue_drops_messages() {
    let addr = spawn_server_with_limits(QueueLimits {
        ttl: Duration::from_secs(60),
        capacity: 1,
    })
    .await;
    let (mut alice, _) = Client::login(addr, ALICE_TOKEN).await;

    let (id, msg) = chat("fits");
    alice.send(&msg).await;
    assert_eq!(alice.recv().await, ServerMessage::ChatQueued { id });

    let (_, msg) = chat("overflow");
    alice.send(&msg).await;
    assert!(matches!(
        alice.recv().await,
        ServerMessage::Error {
            code: ErrorCode::QueueFull,
            ..
        }
    ));
}

#[tokio::test]
async fn expired_queued_messages_are_not_delivered() {
    let addr = spawn_server_with_limits(QueueLimits {
        ttl: Duration::from_millis(100),
        capacity: 10,
    })
    .await;
    let (mut alice, _) = Client::login(addr, ALICE_TOKEN).await;
    let (id, msg) = chat("stale");
    alice.send(&msg).await;
    assert_eq!(alice.recv().await, ServerMessage::ChatQueued { id });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let (mut bob, _) = Client::login(addr, BOB_TOKEN).await;
    assert_eq!(bob.try_recv(Duration::from_millis(300)).await, None);
}
