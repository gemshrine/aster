use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use client_tauri_lib::core::settings::Settings;
use client_tauri_lib::core::signaling::{
    signaling, AckStatus, ConnectionState, CoreEvent, FailReason, SendError, SignalingHandle,
    Timing,
};
use protocol::ErrorCode;
use signaling_server::config::Users;
use signaling_server::hub::{Hub, QueueLimits};
use signaling_server::turn::TurnConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinSet;
use uuid::Uuid;

const ALICE: &str = "alice-token-0123456789abcdef0123456789abcdef";
const BOB: &str = "bob-token-0123456789abcdef0123456789abcdef00";
const WAIT: Duration = Duration::from_secs(5);

fn fast_timing() -> Timing {
    Timing {
        backoff_base: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        connect_timeout: Duration::from_secs(2),
        idle_timeout: Duration::from_secs(5),
    }
}

async fn spawn_server(limits: QueueLimits) -> SocketAddr {
    let users = Users::parse(&format!("alice:{ALICE},bob:{BOB}")).unwrap();
    let turn =
        TurnConfig::new("turn.test", "turn-secret-0123456789abcdef0123456789abcdef").unwrap();
    let hub = Arc::new(Hub::with_limits(users, limits).with_turn(turn));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, signaling_server::app(hub))
            .await
            .unwrap()
    });
    addr
}

struct Core {
    handle: SignalingHandle,
    events: mpsc::UnboundedReceiver<CoreEvent>,
}

impl Core {
    fn start() -> Self {
        let (tx, events) = mpsc::unbounded_channel();
        let (handle, actor) = signaling(tx, fast_timing());
        tokio::spawn(actor);
        Self { handle, events }
    }

    async fn login(addr: SocketAddr, token: &str) -> Self {
        let mut core = Self::start();
        core.connect(addr, token);
        core.wait_for(|e| matches!(e, CoreEvent::State(ConnectionState::Connected { .. })))
            .await;
        core
    }

    fn connect(&self, addr: SocketAddr, token: &str) {
        self.handle
            .connect(Settings::new(&format!("ws://{addr}/ws"), token).unwrap());
    }

    /// Skips events until one matches.
    async fn wait_for(&mut self, pred: impl Fn(&CoreEvent) -> bool) -> CoreEvent {
        let fut = async {
            loop {
                let event = self.events.recv().await.expect("core stopped");
                if pred(&event) {
                    return event;
                }
            }
        };
        tokio::time::timeout(WAIT, fut)
            .await
            .expect("expected event did not arrive")
    }

    /// Asserts nothing matching arrives within `window`.
    async fn assert_no(&mut self, window: Duration, pred: impl Fn(&CoreEvent) -> bool) {
        let deadline = tokio::time::Instant::now() + window;
        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, self.events.recv()).await {
            assert!(!pred(&event), "unexpected event {event:?}");
        }
    }
}

#[tokio::test]
async fn connects_reports_presence_and_ice_servers() {
    let addr = spawn_server(QueueLimits::default()).await;
    let mut alice = Core::start();
    assert_eq!(alice.handle.state(), ConnectionState::NotConfigured);

    alice.connect(addr, ALICE);
    assert_eq!(
        alice.wait_for(|_| true).await,
        CoreEvent::State(ConnectionState::Connecting { attempt: 1 })
    );
    assert_eq!(
        alice.wait_for(|_| true).await,
        CoreEvent::State(ConnectionState::Connected {
            user_id: "alice".into(),
            peer_online: false,
        })
    );
    let ice = alice.handle.ice_servers();
    assert!(ice
        .iter()
        .any(|s| s.urls.iter().any(|u| u.starts_with("turn:turn.test"))));

    let bob = Core::login(addr, BOB).await;
    alice
        .wait_for(|e| *e == CoreEvent::PeerStatus { online: true })
        .await;
    assert_eq!(
        alice.handle.state(),
        ConnectionState::Connected {
            user_id: "alice".into(),
            peer_online: true,
        }
    );

    bob.handle.disconnect();
    alice
        .wait_for(|e| *e == CoreEvent::PeerStatus { online: false })
        .await;
}

#[tokio::test]
async fn chat_is_delivered_or_queued_with_acks() {
    let addr = spawn_server(QueueLimits::default()).await;
    let mut alice = Core::login(addr, ALICE).await;

    let queued = Uuid::new_v4();
    alice
        .handle
        .send_chat(queued, "пока тебя нет".into(), 1)
        .await
        .unwrap();
    alice
        .wait_for(|e| {
            *e == CoreEvent::ChatAck {
                id: queued,
                status: AckStatus::Queued,
            }
        })
        .await;

    let mut bob = Core::login(addr, BOB).await;
    bob.wait_for(|e| matches!(e, CoreEvent::ChatMessage { id, .. } if *id == queued))
        .await;
    alice
        .wait_for(|e| {
            *e == CoreEvent::ChatAck {
                id: queued,
                status: AckStatus::Delivered,
            }
        })
        .await;

    let live = Uuid::new_v4();
    bob.handle
        .send_chat(live, "привет".into(), 2)
        .await
        .unwrap();
    assert_eq!(
        alice
            .wait_for(|e| matches!(e, CoreEvent::ChatMessage { .. }))
            .await,
        CoreEvent::ChatMessage {
            id: live,
            from: "bob".into(),
            body: "привет".into(),
            sent_at: 2,
        }
    );
    bob.wait_for(|e| {
        *e == CoreEvent::ChatAck {
            id: live,
            status: AckStatus::Delivered,
        }
    })
    .await;
}

#[tokio::test]
async fn queue_full_rejection_names_the_dropped_message() {
    let addr = spawn_server(QueueLimits {
        ttl: Duration::from_secs(60),
        capacity: 1,
    })
    .await;
    let mut alice = Core::login(addr, ALICE).await;

    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    alice.handle.send_chat(first, "1".into(), 1).await.unwrap();
    alice.handle.send_chat(second, "2".into(), 2).await.unwrap();

    alice
        .wait_for(|e| {
            *e == CoreEvent::ChatAck {
                id: first,
                status: AckStatus::Queued,
            }
        })
        .await;
    let rejected = alice
        .wait_for(|e| matches!(e, CoreEvent::ChatRejected { .. }))
        .await;
    assert!(matches!(
        rejected,
        CoreEvent::ChatRejected { id: Some(id), code: ErrorCode::QueueFull, .. } if id == second
    ));
}

#[tokio::test]
async fn invalid_token_fails_without_retrying() {
    let addr = spawn_server(QueueLimits::default()).await;
    let mut core = Core::start();
    core.connect(addr, "wrong-token");
    core.wait_for(|e| {
        *e == CoreEvent::State(ConnectionState::Failed {
            reason: FailReason::InvalidToken,
        })
    })
    .await;
    core.assert_no(Duration::from_millis(500), |e| {
        matches!(e, CoreEvent::State(_))
    })
    .await;
}

#[tokio::test]
async fn replaced_session_fails_without_retrying() {
    let addr = spawn_server(QueueLimits::default()).await;
    let mut first = Core::login(addr, ALICE).await;
    let mut second = Core::login(addr, ALICE).await;

    first
        .wait_for(|e| {
            *e == CoreEvent::State(ConnectionState::Failed {
                reason: FailReason::SessionReplaced,
            })
        })
        .await;
    first
        .assert_no(Duration::from_millis(500), |e| {
            matches!(e, CoreEvent::State(_))
        })
        .await;
    second
        .assert_no(Duration::from_millis(100), |e| {
            matches!(e, CoreEvent::State(_))
        })
        .await;
    assert!(matches!(
        second.handle.state(),
        ConnectionState::Connected { .. }
    ));

    // A manual reconnect is the way out of `failed`.
    first.connect(addr, ALICE);
    first
        .wait_for(|e| matches!(e, CoreEvent::State(ConnectionState::Connected { .. })))
        .await;
}

#[tokio::test]
async fn send_chat_requires_a_connection() {
    let core = Core::start();
    assert_eq!(
        core.handle.send_chat(Uuid::new_v4(), "x".into(), 1).await,
        Err(SendError::NotConnected)
    );
}

#[tokio::test]
async fn disconnect_and_connect_again() {
    let addr = spawn_server(QueueLimits::default()).await;
    let mut core = Core::login(addr, ALICE).await;

    core.handle.disconnect();
    core.wait_for(|e| *e == CoreEvent::State(ConnectionState::Disconnected))
        .await;
    assert_eq!(
        core.handle.send_chat(Uuid::new_v4(), "x".into(), 1).await,
        Err(SendError::NotConnected)
    );

    core.connect(addr, ALICE);
    core.wait_for(|e| matches!(e, CoreEvent::State(ConnectionState::Connected { .. })))
        .await;
}

#[tokio::test]
async fn unreachable_server_backs_off_with_growing_attempts() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let mut core = Core::start();
    core.connect(addr, ALICE);
    for attempt in 1..=3 {
        core.wait_for(|e| {
            matches!(e, CoreEvent::State(ConnectionState::Reconnecting { attempt: a, .. }) if *a == attempt)
        })
        .await;
    }
}

/// TCP proxy that can drop every proxied connection to simulate a network cut.
struct Proxy {
    addr: SocketAddr,
    up: Arc<AtomicBool>,
    conns: Arc<Mutex<JoinSet<()>>>,
}

impl Proxy {
    async fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let up = Arc::new(AtomicBool::new(true));
        let conns = Arc::new(Mutex::new(JoinSet::new()));
        let (up_task, conns_task) = (up.clone(), conns.clone());
        tokio::spawn(async move {
            loop {
                let (mut client, _) = listener.accept().await.unwrap();
                if !up_task.load(Ordering::SeqCst) {
                    continue;
                }
                conns_task.lock().await.spawn(async move {
                    if let Ok(mut server) = TcpStream::connect(upstream).await {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                    }
                });
            }
        });
        Self { addr, up, conns }
    }

    async fn cut(&self) {
        self.up.store(false, Ordering::SeqCst);
        self.conns.lock().await.abort_all();
    }

    fn restore(&self) {
        self.up.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn reconnects_after_connection_loss() {
    let server = spawn_server(QueueLimits::default()).await;
    let proxy = Proxy::start(server).await;
    let mut core = Core::login(proxy.addr, ALICE).await;

    proxy.cut().await;
    core.wait_for(|e| {
        matches!(
            e,
            CoreEvent::State(ConnectionState::Reconnecting { attempt: 1, .. })
        )
    })
    .await;
    core.wait_for(|e| {
        matches!(
            e,
            CoreEvent::State(ConnectionState::Reconnecting { attempt: 2, .. })
        )
    })
    .await;

    proxy.restore();
    core.wait_for(|e| matches!(e, CoreEvent::State(ConnectionState::Connected { .. })))
        .await;

    // The backoff streak resets once connected again.
    proxy.cut().await;
    core.wait_for(|e| matches!(e, CoreEvent::State(ConnectionState::Reconnecting { .. })))
        .await;
    assert!(matches!(
        core.handle.state(),
        ConnectionState::Reconnecting { attempt: 1, .. }
            | ConnectionState::Connecting { attempt: 2 }
    ));
}
