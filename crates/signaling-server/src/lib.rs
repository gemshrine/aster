pub mod config;
pub mod hub;
pub mod turn;
mod ws;

use std::sync::Arc;

use axum::{Router, routing::get};

use crate::hub::Hub;

pub fn app(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws::ws_handler))
        .with_state(hub)
}

async fn health() -> &'static str {
    "ok"
}
