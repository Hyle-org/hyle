//! Public API for interacting with the node.

use crate::{
    bus::SharedMessageBus, log_error, module_bus_client, module_handle_messages, modules::Module,
};
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
use prometheus::{Encoder, Registry, TextEncoder};
use sdk::{api::NodeInfo, *};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

pub use client_sdk::contract_indexer::AppError;
pub use client_sdk::rest_client as client;

module_bus_client! {
    struct RestBusClient {
    }
}

pub struct RestApiRunContext {
    pub port: u16,
    pub info: NodeInfo,
    pub bus: SharedMessageBus,
    pub router: Router,
    pub registry: Registry,
    pub metrics_layer: Option<HttpMetricsLayer>,
    pub max_body_size: usize,
    pub openapi: utoipa::openapi::OpenApi,
}

impl RestApiRunContext {
    pub fn new(
        port: u16,
        info: NodeInfo,
        bus: SharedMessageBus,
        router: Router,
        metrics_layer: Option<HttpMetricsLayer>,
        max_body_size: usize,
        openapi: utoipa::openapi::OpenApi,
    ) -> RestApiRunContext {
        Self {
            port,
            info,
            bus,
            router,
            registry: Registry::new(),
            metrics_layer,
            max_body_size,
            openapi,
        }
    }
    pub fn with_registry(self, registry: Registry) -> Self {
        Self { registry, ..self }
    }
}

pub struct RouterState {
    info: NodeInfo,
    registry: Registry,
}

pub struct RestApi {
    port: u16,
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
                .with_state(RouterState {
                    info: ctx.info,
                    registry: ctx.registry,
                }),
        );
        let app = match ctx.metrics_layer {
            Some(ml) => app.layer(ml),
            None => app,
        };
        let app = app
            .layer(DefaultBodyLimit::max(ctx.max_body_size)) // 10 MB
            .layer(tower_http::cors::CorsLayer::permissive())
            .layer(axum::middleware::from_fn(request_logger));
        Ok(RestApi {
            port: ctx.port,
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

pub async fn get_metrics(State(s): State<RouterState>) -> Result<impl IntoResponse, AppError> {
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();
    encoder.encode(&s.registry.gather(), &mut buffer)?;
    String::from_utf8(buffer).map_err(Into::into)
}

impl RestApi {
    pub async fn serve(&mut self) -> Result<()> {
        info!(
            "📡  Starting {} module, listening on port {}",
            std::any::type_name::<Self>(),
            self.port
        );

        let listener = hyle_net::net::bind_tcp_listener(self.port)
            .await
            .context("Starting rest server")?;

        #[allow(
            clippy::expect_used,
            reason = "app is guaranteed to be set during initialization"
        )]
        let app = self.app.take().expect("app is not set");

        // On module shutdown, we want to shutdown the axum server and wait for its shutdown to complete.
        let axum_cancel_token = CancellationToken::new();
        let axum_server = tokio::spawn({
            let token = axum_cancel_token.clone();
            async move {
                log_error!(
                    axum::serve(listener, app)
                        .with_graceful_shutdown(async move {
                            token.cancelled().await;
                        })
                        .await,
                    "serving Axum"
                )?;
                Ok::<(), anyhow::Error>(())
            }
        });
        module_handle_messages! {
            on_bus self.bus,
            delay_shutdown_until {
                // When the module tries to shutdown it'll cancel the token
                // and then we actually exit the loop when axum is done.
                axum_cancel_token.cancel();
                axum_server.is_finished()
            },
        };

        Ok(())
    }
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        Self {
            info: self.info.clone(),
            registry: self.registry.clone(),
        }
    }
}
