use std::process::ExitCode;
use std::sync::Arc;

use signaling_server::app;
use signaling_server::config::Users;
use signaling_server::hub::Hub;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    let users = match std::env::var("ASTER_USERS")
        .map_err(|_| "ASTER_USERS is not set (expected `id:token,id:token`)".to_string())
        .and_then(|raw| Users::parse(&raw))
    {
        Ok(users) => users,
        Err(err) => {
            tracing::error!("invalid configuration: {err}");
            return ExitCode::FAILURE;
        }
    };

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = format!("0.0.0.0:{port}");

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!("failed to bind {addr}: {err}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        %addr,
        protocol_version = protocol::PROTOCOL_VERSION,
        "signaling-server listening"
    );

    if let Err(err) = axum::serve(listener, app(Arc::new(Hub::new(users)))).await {
        tracing::error!("server error: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
