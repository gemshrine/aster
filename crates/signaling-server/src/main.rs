use axum::{Router, routing::get};

/// Builds the axum [`Router`] for the signaling server.
///
/// Split out from `main` so it can be exercised directly in tests without
/// binding a real socket.
pub fn app() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = format!("0.0.0.0:{port}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|err| panic!("failed to bind {addr}: {err}"));
    tracing::info!(
        %addr,
        protocol_version = protocol::PROTOCOL_VERSION,
        "signaling-server listening"
    );

    axum::serve(listener, app())
        .await
        .expect("signaling-server crashed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_check_returns_ok() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
