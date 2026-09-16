#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use client_tauri_lib::core::signaling::Timing;
use signaling_server::config::Users;
use signaling_server::hub::{Hub, QueueLimits};
use signaling_server::turn::TurnConfig;
use tokio::net::TcpListener;

pub const ALICE: &str = "alice-token-0123456789abcdef0123456789abcdef";
pub const BOB: &str = "bob-token-0123456789abcdef0123456789abcdef00";
pub const WAIT: Duration = Duration::from_secs(5);

pub fn fast_timing() -> Timing {
    Timing {
        backoff_base: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        connect_timeout: Duration::from_secs(2),
        idle_timeout: Duration::from_secs(5),
        ping_interval: Duration::from_secs(5),
    }
}

pub async fn spawn_server(limits: QueueLimits) -> SocketAddr {
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
