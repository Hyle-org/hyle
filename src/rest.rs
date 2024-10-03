//! Public API for interacting with the node.

use crate::{
    bus::{bus_client, command_response::Query, SharedMessageBus},
    model::ContractName,
    node_state::model::Contract,
    tools::mock_workflow::RunScenario,
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

pub mod client;
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
}

pub struct RestApiRunContext {
    pub rest_addr: String,
    pub bus: SharedMessageBus,
    pub router: Router,
    pub metrics_layer: HttpMetricsLayer,
}

pub struct RestApi {}
impl Module for RestApi {
    fn name() -> &'static str {
        "RestApi"
    }

    type Context = RestApiRunContext;

    async fn build(ctx: &Self::Context) -> Result<Self> {
        // TODO: do better, splitting router from start is harder than expected
        let _ = RestBusClient::new_from_bus(ctx.bus.new_handle()).await;
        Ok(RestApi {})
    }

    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        self.serve(
            ctx.rest_addr,
            ctx.metrics_layer,
            ctx.bus.new_handle(),
            ctx.router,
        )
    }
}

impl RestApi {
    pub async fn serve(
        &self,
        rest_addr: String,
        metrics_layer: HttpMetricsLayer,
        bus: SharedMessageBus,
        router: Router,
    ) -> Result<()> {
        info!("rest listening on {}", rest_addr);
        let app = router
            .merge(
                Router::new()
                    .route("/v1/contract/:name", get(endpoints::get_contract))
                    .route(
                        "/v1/contract/register",
                        post(endpoints::send_contract_transaction),
                    )
                    .route("/v1/tx/send/blob", post(endpoints::send_blob_transaction))
                    .route("/v1/tx/send/proof", post(endpoints::send_proof_transaction))
                    .route("/v1/tools/run_scenario", post(endpoints::run_scenario))
                    .with_state(RouterState {
                        bus: RestBusClient::new_from_bus(bus).await,
                    })
                    .nest("/v1", metrics_layer.routes()),
            )
            .layer(metrics_layer)
            .layer(tower_http::cors::CorsLayer::permissive());

        let listener = tokio::net::TcpListener::bind(rest_addr)
            .await
            .context("Starting rest server")?;

        // TODO: racelayer should be added only in "dev mode"
        axum::serve(listener, app.layer(TraceLayer::new_for_http()))
            .await
            .context("Starting rest server")
    }
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
