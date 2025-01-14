//! Public API for interacting with the node.

use anyhow::{Context, Result};
pub use axum::Router;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use axum_otel_metrics::HttpMetricsLayer;
use prometheus::{Encoder, TextEncoder};
use reqwest::StatusCode;
use tokio::time::Instant;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::{bus::SharedMessageBus, module_handle_messages, utils::modules::module_bus_client};
use crate::{model::rest::NodeInfo, utils::modules::Module};

pub use crate::tools::rest_api_client as client;

module_bus_client! {
    struct RestBusClient {
    }
}

pub struct RestApiRunContext {
    pub rest_addr: String,
    pub info: NodeInfo,
    pub bus: SharedMessageBus,
    pub router: Router,
    pub metrics_layer: HttpMetricsLayer,
    pub max_body_size: usize,
}

pub struct RouterState {
    info: NodeInfo,
}

pub struct RestApi {
    rest_addr: String,
    app: Option<Router>,
    bus: RestBusClient,
}

impl Module for RestApi {
    type Context = RestApiRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let app = ctx
            .router
            .merge(
                Router::new()
                    .route("/v1/info", get(get_info))
                    .route("/v1/metrics", get(get_metrics))
                    .with_state(RouterState { info: ctx.info }),
            )
            .layer(ctx.metrics_layer)
            .layer(DefaultBodyLimit::max(ctx.max_body_size)) // 10 MB
            .layer(tower_http::cors::CorsLayer::permissive())
            .layer(axum::middleware::from_fn(request_logger))
            //.layer(TraceLayer::new_for_http())
        ;
        Ok(RestApi {
            rest_addr: ctx.rest_addr.clone(),
            app: Some(app),
            bus: RestBusClient::new_from_bus(ctx.bus.new_handle()).await,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.serve()
    }
}

async fn request_logger(req: Request<Body>, next: Next) -> impl IntoResponse {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let start_time = Instant::now();

    // Passer la requête au prochain middleware ou au gestionnaire
    let response = next.run(req).await;

    let status = response.status();
    let elapsed_time = start_time.elapsed();

    // Will log like:
    // [GET] /v1/indexer/contract/amm - 200 OK (1484 μs)
    info!(
        "[{}] {} - {} ({} μs)",
        method,
        uri,
        status,
        elapsed_time.as_micros()
    );

    response
}

pub async fn get_info(State(state): State<RouterState>) -> Result<impl IntoResponse, AppError> {
    Ok(Json(state.info))
}

pub async fn get_metrics(State(_): State<RouterState>) -> Result<impl IntoResponse, AppError> {
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();
    encoder.encode(&prometheus::gather(), &mut buffer)?;
    String::from_utf8(buffer).map_err(Into::into)
}

impl RestApi {
    pub async fn serve(&mut self) -> Result<()> {
        info!(
            "📡  Starting RestApi module, listening on {}",
            self.rest_addr
        );

        module_handle_messages! {
            on_bus self.bus,
            _ = axum::serve(
                tokio::net::TcpListener::bind(&self.rest_addr)
                    .await
                    .context("Starting rest server")?,
                #[allow(clippy::expect_used, reason="incorrect setup logic")]
                self.app.take().expect("app is not set")
            ) => { }
        }

        Ok(())
    }
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        Self {
            info: self.info.clone(),
        }
    }
}

// Make our own error that wraps `anyhow::Error`.
pub struct AppError(pub StatusCode, pub anyhow::Error);

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
