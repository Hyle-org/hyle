//! Public API for interacting with the node.

use crate::{bus::SharedMessageBus, history::History, utils::conf::SharedConf};
use anyhow::{Context, Result};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use axum_otel_metrics::HttpMetricsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

pub mod endpoints;

pub struct RouterState {
    pub bus: SharedMessageBus,
    pub history: History,
}

pub async fn rest_server(
    config: SharedConf,
    bus: SharedMessageBus,
    metrics_layer: HttpMetricsLayer,
    history: History,
) -> Result<()> {
    info!("rest listening on {}", config.rest_addr());
    let app = Router::new()
        .nest("/v1", metrics_layer.routes())
        .route("/v1/contract/:name", get(endpoints::get_contract))
        .route(
            "/v1/contract/register",
            post(endpoints::send_contract_transaction),
        )
        .route("/v1/tx/send/blob", post(endpoints::send_blob_transaction))
        .route("/v1/tx/send/proof", post(endpoints::send_proof_transaction))
        .route("/v1/tools/run_scenario", post(endpoints::run_scenario))
        .nest("/v1/indexer", History::api())
        .layer(metrics_layer)
        .with_state(RouterState { bus, history });

    let listener = tokio::net::TcpListener::bind(config.rest_addr())
        .await
        .context("Starting rest server")?;

    // TODO: racelayer should be added only in "dev mode"
    axum::serve(listener, app.layer(TraceLayer::new_for_http()))
        .await
        .context("Starting rest server")
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        Self {
            bus: self.bus.new_handle(),
            history: self.history.share(),
        }
    }
}

// Make our own error that wraps `anyhow::Error`.
pub struct AppError(StatusCode, anyhow::Error);

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
