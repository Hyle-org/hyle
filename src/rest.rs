//! Public API for interacting with the node.

use crate::{
    bus::SharedMessageBus,
    bus::{bus_client, command_response::Query},
    history::History,
    model::SharedRunContext,
    node_state::{model::Contract, NodeStateQuery},
    tools::mock_workflow::RunScenario,
    utils::conf::SharedConf,
    utils::modules::Module,
};
use anyhow::{Context, Result};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use axum_otel_metrics::HttpMetricsLayer;
use endpoints::RestApiMessage;
use tower_http::trace::TraceLayer;
use tracing::info;

pub mod endpoints;

bus_client! {
struct RestBusClient {
    sender(RestApiMessage),
    sender(RunScenario),
    sender(Query<NodeStateQuery, Contract>),
}
}

pub struct RouterState {
    bus: RestBusClient,
    pub history: History,
}

pub struct RestApiRunContext {
    pub ctx: SharedRunContext,
    pub metrics_layer: HttpMetricsLayer,
    pub history: History,
}

pub struct RestApi {}
impl Module for RestApi {
    fn name() -> &'static str {
        "RestApi"
    }

    type Context = RestApiRunContext;

    async fn build(_ctx: &Self::Context) -> Result<Self> {
        Ok(RestApi {})
    }

    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        rest_server(
            ctx.ctx.config.clone(),
            ctx.ctx.bus.new_handle(),
            ctx.metrics_layer,
            ctx.history,
        )
    }
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
        .nest("/v1/history", History::api())
        .layer(metrics_layer)
        .with_state(RouterState {
            bus: RestBusClient::new_from_bus(bus).await,
            history,
        });

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
        use crate::utils::generic_tuple::Pick;
        Self {
            bus: RestBusClient::new(
                Pick::<tokio::sync::broadcast::Sender<RestApiMessage>>::get(&self.bus).clone(),
                Pick::<tokio::sync::broadcast::Sender<RunScenario>>::get(&self.bus).clone(),
                Pick::<tokio::sync::broadcast::Sender<Query<NodeStateQuery, Contract>>>::get(
                    &self.bus,
                )
                .clone(),
            ),
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
