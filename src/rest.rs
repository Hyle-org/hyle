use crate::{bus::SharedMessageBus, indexer::Indexer, utils::conf::SharedConf};
use anyhow::{Context, Result};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use tracing::info;

mod endpoints;

pub struct RouterState {
    pub bus: SharedMessageBus,
    pub idxr: Indexer,
}

pub async fn rest_server(config: SharedConf, bus: SharedMessageBus, idxr: Indexer) -> Result<()> {
    info!("rest listening on {}", config.rest_addr());
    let app = Router::new()
        .route("/v1/contract/:name", get(endpoints::get_contract))
        .route(
            "/v1/contract/register",
            post(endpoints::send_contract_transaction),
        )
        .route("/v1/tx/send/blob", post(endpoints::send_blob_transaction))
        .route("/v1/tx/send/proof", post(endpoints::send_proof_transaction))
        .route("/v1/tx/get/:tx_hash", get(endpoints::get_transaction))
        .route("/v1/block/height/:height", get(endpoints::get_block))
        .route("/v1/block/current", get(endpoints::get_current_block))
        .with_state(RouterState { bus, idxr });

    let listener = tokio::net::TcpListener::bind(config.rest_addr())
        .await
        .context("Starting rest server")?;

    axum::serve(listener, app)
        .await
        .context("Starting rest server")
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        Self {
            bus: self.bus.new_handle(),
            idxr: self.idxr.clone(),
        }
    }
}

// Make our own error that wraps `anyhow::Error`.
struct AppError(StatusCode, anyhow::Error);

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, format!("{}", self.1)).into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, err.into())
    }
}
