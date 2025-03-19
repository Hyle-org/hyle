//! Public API for interacting with the node.

use std::net::{IpAddr, Ipv4Addr};

use anyhow::{Context, Result};
pub use axum::Router;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::Request,
    middleware::Next,
    response::IntoResponse,
    routing::get,
    Json,
};
use axum_otel_metrics::HttpMetricsLayer;
use hyle_model::api::*;
use hyle_model::*;
use prometheus::{Encoder, TextEncoder};
use tokio::time::Instant;
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::utils::modules::Module;
use crate::{bus::SharedMessageBus, module_handle_messages, utils::modules::module_bus_client};

pub use client_sdk::contract_indexer::AppError;
pub use client_sdk::rest_client as client;

module_bus_client! {
    struct RestBusClient {
    }
}

pub struct RestApiRunContext {
    pub rest_addr: String,
    pub info: NodeInfo,
    pub bus: SharedMessageBus,
    pub router: Router,
    pub metrics_layer: Option<HttpMetricsLayer>,
    pub max_body_size: usize,
    pub openapi: utoipa::openapi::OpenApi,
}

pub struct RouterState {
    info: NodeInfo,
}

pub struct RestApi {
    rest_addr: String,
    app: Option<Router>,
    bus: RestBusClient,
}

#[derive(OpenApi)]
#[openapi(
    info(
        description = "Hyle Node API",
        title = "Hyle Node API",
    ),
    // When opening the swagger, if on some endpoint you get the error:
    // Could not resolve reference: JSON Pointer evaluation failed while evaluating token "BlobIndex" against an ObjectElement
    // then it means you need to add it to this list.
    // More details here: https://github.com/juhaku/utoipa/issues/894
    components(schemas(BlobIndex, RegisterContractEffect))
)]
pub struct ApiDoc;

impl Module for RestApi {
    type Context = RestApiRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let app = ctx.router.merge(
            Router::new()
                .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ctx.openapi))
                .route("/v1/info", get(get_info))
                .route("/v1/metrics", get(get_metrics))
                .with_state(RouterState { info: ctx.info }),
        );
        let app = match ctx.metrics_layer {
            Some(ml) => app.layer(ml),
            None => app,
        };
        let app = app
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

        let addr = (
            IpAddr::from(Ipv4Addr::UNSPECIFIED),
            self.rest_addr.split(":").last().unwrap().parse().unwrap(),
        );

        module_handle_messages! {
            on_bus self.bus,
            _ = axum::serve(
                tokio::net::TcpListener::bind(&addr)
                    .await
                    .context("Starting rest server")?,
                #[allow(clippy::expect_used, reason="incorrect setup logic")]
                self.app.take().expect("app is not set")
            ) => { }
        };

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
