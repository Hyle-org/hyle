//! Public API for interacting with the node.

use crate::{
    bus::{bus_client, command_response::Query, SharedMessageBus},
    history::{History, HistoryState},
    model::{ContractName, SharedRunContext},
    node_state::model::Contract,
    tools::mock_workflow::RunScenario,
    utils::{conf::SharedConf, modules::Module},
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
    sender(Query<ContractName, Contract>),
}
}

pub struct RouterState {
    bus: RestBusClient,
    pub history: HistoryState,
}

pub struct RestApiRunContext {
    pub ctx: SharedRunContext,
    pub metrics_layer: HttpMetricsLayer,
    pub history: HistoryState,
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
    history: HistoryState,
) -> Result<()> {
    info!("rest listening on {}", config.rest_addr());

    let mut server = oasgen::Server::axum();
    server.openapi.info.title = "Hyle Node API".to_string();
    let mut server = server
        .get("/v1/contract/:name", endpoints::get_contract)
        .post(
            "/v1/contract/register",
            endpoints::send_contract_transaction,
        )
        .post("/v1/tx/send/blob", endpoints::send_blob_transaction)
        .post("/v1/tx/send/proof", endpoints::send_proof_transaction)
        .post("/v1/tools/run_scenario", endpoints::run_scenario)
        .route_json_spec("/openapi.json")
        .swagger_ui("/openapi/");

    let (history_openapi, history_router) = History::api();

    server
        .openapi
        .paths
        .paths
        .append(&mut history_openapi.paths.paths.clone());

    let app = Router::new()
        .nest("/v1", metrics_layer.routes())
        .merge(server.freeze().into_router())
        .nest("/v1/history", history_router)
        .layer(metrics_layer)
        .layer(tower_http::cors::CorsLayer::permissive())
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
        use crate::utils::static_type_map::Pick;
        Self {
            bus: RestBusClient::new(
                Pick::<tokio::sync::broadcast::Sender<RestApiMessage>>::get(&self.bus).clone(),
                Pick::<tokio::sync::broadcast::Sender<RunScenario>>::get(&self.bus).clone(),
                Pick::<tokio::sync::broadcast::Sender<Query<ContractName, Contract>>>::get(
                    &self.bus,
                )
                .clone(),
            ),
            history: self.history.clone(),
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
