use std::process::ExitCode;
use std::sync::Arc;

use signaling_server::app;
use signaling_server::config::Users;
use signaling_server::hub::Hub;
use signaling_server::turn::TurnConfig;

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

    let turn = match (std::env::var("TURN_HOST"), std::env::var("TURN_SECRET")) {
        (Ok(host), Ok(secret)) => match TurnConfig::new(&host, &secret) {
            Ok(turn) => Some(turn),
            Err(err) => {
                tracing::error!("invalid configuration: {err}");
                return ExitCode::FAILURE;
            }
        },
        (Err(_), Err(_)) => {
            tracing::warn!("TURN_HOST/TURN_SECRET not set, clients get no ICE servers");
            None
        }
        _ => {
            tracing::error!("invalid configuration: set both TURN_HOST and TURN_SECRET or neither");
            return ExitCode::FAILURE;
        }
    };

    let mut hub = Hub::new(users);
    if let Some(turn) = turn {
        hub = hub.with_turn(turn);
    }

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

    if let Err(err) = axum::serve(listener, app(Arc::new(hub))).await {
        tracing::error!("server error: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
