#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};
use signaling_server::config::Users;
use signaling_server::hub::{Hub, QueueLimits};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

pub const ALICE_TOKEN: &str = "alice-token-0123456789abcdef0123456789abcdef";
pub const BOB_TOKEN: &str = "bob-token-0123456789abcdef0123456789abcdef00";

const RECV_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn spawn_server() -> SocketAddr {
    spawn_server_with_limits(QueueLimits::default()).await
}

pub async fn spawn_server_with_limits(limits: QueueLimits) -> SocketAddr {
    let users = Users::parse(&format!("alice:{ALICE_TOKEN},bob:{BOB_TOKEN}")).unwrap();
    let hub = Arc::new(Hub::with_limits(users, limits));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, signaling_server::app(hub))
            .await
            .unwrap();
    });
    addr
}

pub struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Client {
    pub async fn connect(addr: SocketAddr) -> Self {
        let (ws, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        Self { ws }
    }

    /// Connects and completes the handshake, returning the `Welcome` message.
    pub async fn login(addr: SocketAddr, token: &str) -> (Self, ServerMessage) {
        let mut client = Self::connect(addr).await;
        client.hello(PROTOCOL_VERSION, token).await;
        let welcome = client.recv().await;
        assert!(
            matches!(welcome, ServerMessage::Welcome { .. }),
            "expected welcome, got {welcome:?}"
        );
        (client, welcome)
    }

    pub async fn hello(&mut self, protocol_version: u32, token: &str) {
        self.send(&ClientMessage::Hello {
            protocol_version,
            auth_token: token.into(),
        })
        .await;
    }

    pub async fn send(&mut self, msg: &ClientMessage) {
        self.send_raw(&serde_json::to_string(msg).unwrap()).await;
    }

    pub async fn send_raw(&mut self, text: &str) {
        self.ws.send(Message::text(text)).await.unwrap();
    }

    pub async fn recv(&mut self) -> ServerMessage {
        self.try_recv(RECV_TIMEOUT)
            .await
            .expect("expected a server message")
    }

    /// Next server message, or `None` if nothing arrives within `wait` or the
    /// connection closes.
    pub async fn try_recv(&mut self, wait: Duration) -> Option<ServerMessage> {
        loop {
            match tokio::time::timeout(wait, self.ws.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    return Some(serde_json::from_str(&text).unwrap());
                }
                Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
                _ => return None,
            }
        }
    }

    /// Asserts the server closes the connection.
    pub async fn expect_closed(&mut self) {
        loop {
            match tokio::time::timeout(RECV_TIMEOUT, self.ws.next()).await {
                Ok(None | Some(Err(_)) | Some(Ok(Message::Close(_)))) => return,
                Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
                Ok(Some(Ok(other))) => panic!("expected close, got {other:?}"),
                Err(_) => panic!("connection was not closed"),
            }
        }
    }

    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}
