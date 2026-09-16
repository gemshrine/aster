mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use client_tauri_lib::core::chat::{Chat, ChatStore, Direction, Message, Status};
use client_tauri_lib::core::settings::Settings;
use client_tauri_lib::core::signaling::{signaling, ConnectionState, SignalingHandle};
use common::{fast_timing, spawn_server, ALICE, BOB, WAIT};
use signaling_server::hub::QueueLimits;
use tokio::sync::mpsc;

/// Signaling actor + history, wired the same way as the Tauri app.
struct Stack {
    signaling: SignalingHandle,
    chat: Arc<Chat>,
    upserts: mpsc::UnboundedReceiver<Message>,
}

impl Stack {
    fn start(store: ChatStore) -> Self {
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let (signaling, actor) = signaling(events_tx, fast_timing());
        tokio::spawn(actor);

        let (upserts_tx, upserts) = mpsc::unbounded_channel();
        let chat = Arc::new(Chat::new(store, signaling.clone(), upserts_tx));
        let chat_events = chat.clone();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                chat_events.handle_event(&event).await;
            }
        });
        Self {
            signaling,
            chat,
            upserts,
        }
    }

    fn fresh() -> Self {
        Self::start(ChatStore::open_in_memory().unwrap())
    }

    async fn connect(&self, addr: SocketAddr, token: &str) {
        connect(&self.signaling, addr, token).await;
    }

    async fn wait_upsert(&mut self, pred: impl Fn(&Message) -> bool) -> Message {
        let fut = async {
            loop {
                let message = self.upserts.recv().await.expect("chat stopped");
                if pred(&message) {
                    return message;
                }
            }
        };
        tokio::time::timeout(WAIT, fut)
            .await
            .expect("expected upsert did not arrive")
    }

    async fn assert_no_upsert(&mut self, window: Duration, pred: impl Fn(&Message) -> bool) {
        let deadline = tokio::time::Instant::now() + window;
        while let Ok(Some(message)) = tokio::time::timeout_at(deadline, self.upserts.recv()).await {
            assert!(!pred(&message), "unexpected upsert {message:?}");
        }
    }
}

async fn connect(signaling: &SignalingHandle, addr: SocketAddr, token: &str) {
    signaling.connect(Settings::new(&format!("ws://{addr}/ws"), token).unwrap());
    tokio::time::timeout(WAIT, async {
        while !matches!(signaling.state(), ConnectionState::Connected { .. }) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("did not connect");
}

fn status(status: Status) -> impl Fn(&Message) -> bool {
    move |m| m.status == Some(status)
}

#[tokio::test]
async fn offline_message_is_resent_then_queued_then_delivered() {
    let addr = spawn_server(QueueLimits::default()).await;
    let mut alice = Stack::fresh();

    let sent = alice.chat.send_message("написал без сети").await.unwrap();
    assert_eq!(sent.status, Some(Status::Sending));
    alice.wait_upsert(status(Status::Sending)).await;

    alice.connect(addr, ALICE).await;
    alice.wait_upsert(status(Status::Sent)).await;
    alice.wait_upsert(status(Status::Queued)).await;

    let mut bob = Stack::fresh();
    bob.connect(addr, BOB).await;
    let received = bob.wait_upsert(|m| m.direction == Direction::In).await;
    assert_eq!(received.id, sent.id);
    assert_eq!(received.body, "написал без сети");
    assert_eq!(received.status, None);

    let delivered = alice.wait_upsert(status(Status::Delivered)).await;
    assert_eq!(delivered.id, sent.id);

    assert_eq!(bob.chat.load_history(None, 50).unwrap(), vec![received]);
    assert_eq!(alice.chat.load_history(None, 50).unwrap(), vec![delivered]);
}

#[tokio::test]
async fn resent_message_is_not_duplicated_for_the_recipient() {
    let addr = spawn_server(QueueLimits::default()).await;
    let mut bob = Stack::fresh();
    bob.connect(addr, BOB).await;

    // Alice's history says `sent` without an ack, but the message did reach Bob.
    let store = ChatStore::open_in_memory().unwrap();
    let message = store.insert_outgoing("дошло, но ack потерялся", 1).unwrap();
    store.advance(message.id, Status::Sent).unwrap();

    let (tx, _events) = mpsc::unbounded_channel();
    let (first_session, actor) = signaling(tx, fast_timing());
    tokio::spawn(actor);
    connect(&first_session, addr, ALICE).await;
    first_session
        .send_chat(message.id, message.body.clone(), message.sent_at)
        .await
        .unwrap();
    bob.wait_upsert(|m| m.id == message.id).await;
    first_session.disconnect();

    let mut alice = Stack::start(store);
    alice.connect(addr, ALICE).await;
    alice.wait_upsert(status(Status::Delivered)).await;

    bob.assert_no_upsert(Duration::from_millis(300), |m| m.id == message.id)
        .await;
    assert_eq!(bob.chat.load_history(None, 50).unwrap().len(), 1);
}

#[tokio::test]
async fn queue_full_fails_and_retry_delivers() {
    let addr = spawn_server(QueueLimits {
        ttl: Duration::from_secs(60),
        capacity: 1,
    })
    .await;
    let mut alice = Stack::fresh();
    alice.connect(addr, ALICE).await;

    let first = alice.chat.send_message("влезло").await.unwrap();
    alice
        .wait_upsert(|m| m.id == first.id && m.status == Some(Status::Queued))
        .await;

    let second = alice.chat.send_message("не влезло").await.unwrap();
    alice
        .wait_upsert(|m| m.id == second.id && m.status == Some(Status::Failed))
        .await;

    let mut bob = Stack::fresh();
    bob.connect(addr, BOB).await;
    bob.wait_upsert(|m| m.id == first.id).await;
    alice
        .wait_upsert(|m| m.id == first.id && m.status == Some(Status::Delivered))
        .await;

    let retried = alice.chat.retry_message(second.id).await.unwrap();
    assert_ne!(retried.status, Some(Status::Failed));
    bob.wait_upsert(|m| m.id == second.id).await;
    alice
        .wait_upsert(|m| m.id == second.id && m.status == Some(Status::Delivered))
        .await;

    let bodies: Vec<_> = bob
        .chat
        .load_history(None, 50)
        .unwrap()
        .into_iter()
        .map(|m| m.body)
        .collect();
    assert_eq!(bodies, ["влезло", "не влезло"]);
}
