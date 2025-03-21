//! Public API for interacting with the node.

use std::net::Ipv4Addr;

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
use tokio_util::sync::CancellationToken;
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{bus::SharedMessageBus, module_handle_messages, utils::modules::module_bus_client};
use crate::{log_error, utils::modules::Module};

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
    pub metrics_layer: Option<HttpMetricsLayer>,
    pub max_body_size: usize,
    pub openapi: utoipa::openapi::OpenApi,
}

pub struct RouterState {
    info: NodeInfo,
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

pub async fn get_metrics(State(_): State<RouterState>) -> Result<impl IntoResponse, AppError> {
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();
    encoder.encode(&prometheus::gather(), &mut buffer)?;
    String::from_utf8(buffer).map_err(Into::into)
}

impl RestApi {
    pub async fn serve(&mut self) -> Result<()> {
        info!(
            "📡  Starting {} module, listening on port {}",
            std::any::type_name::<Self>(),
            self.port
        );

        let listener = tokio::net::TcpListener::bind(&(Ipv4Addr::UNSPECIFIED, self.port))
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bus::bus_client,
        data_availability::DataAvailability,
        genesis::Genesis,
        mempool::api::RestApiMessage,
        model::SharedRunContext,
        node_state::module::NodeStateModule,
        p2p::P2P,
        single_node_consensus::SingleNodeConsensus,
        tcp_server::TcpServer,
        utils::{
            integration_test::NodeIntegrationCtxBuilder,
            modules::{signal::ShutdownModule, Module},
        },
    };
    use anyhow::Result;
    use client_sdk::rest_client::NodeApiHttpClient;
    use hyle_model::BlobTransaction;
    use std::time::Duration;

    bus_client! {
        struct MockBusClient {
            receiver(RestApiMessage),
            receiver(ShutdownModule),
        }
    }

    // Mock module that listens to REST API
    struct RestApiListener {
        bus: MockBusClient,
    }

    impl Module for RestApiListener {
        type Context = SharedRunContext;

        async fn build(ctx: Self::Context) -> Result<Self> {
            Ok(RestApiListener {
                bus: MockBusClient::new_from_bus(ctx.common.bus.new_handle()).await,
            })
        }

        async fn run(&mut self) -> Result<()> {
            module_handle_messages! {
                on_bus self.bus,
                listen<RestApiMessage> msg => {
                    info!("Received REST API message: {:?}", msg);
                    // Just listen to messages to skip the shutdown timer.
                }
            };

            Ok(())
        }
    }

    #[test_log::test(tokio::test(flavor = "multi_thread", worker_threads = 2))]
    async fn test_rest_api_shutdown_with_mocked_modules() {
        // Create a new integration test context with all modules mocked except REST API
        let builder = NodeIntegrationCtxBuilder::new().await;
        let rest_client = builder.conf.rest_server_port;

        // Mock Genesis with our RestApiListener, and skip other modules except mempool (for its API)
        let builder = builder
            .with_mock::<Genesis, RestApiListener>()
            .skip::<SingleNodeConsensus>()
            .skip::<DataAvailability>()
            .skip::<NodeStateModule>()
            .skip::<P2P>()
            .skip::<TcpServer>();

        let node = builder.build().await.expect("Failed to build node");

        let client = NodeApiHttpClient::new(format!("http://localhost:{}", rest_client))
            .expect("Failed to create client");

        node.wait_for_rest_api(&client).await.unwrap();

        // Spawn a task to send requests
        let request_handle = tokio::spawn({
            let client = NodeApiHttpClient::new(format!("http://localhost:{}", rest_client))
                .expect("Failed to create client");
            let dummy_tx = BlobTransaction::new(
                "test.identity",
                vec![Blob {
                    contract_name: "identity".into(),
                    data: BlobData(vec![0, 1, 2, 3]),
                }],
            );
            async move {
                let mut request_count = 0;
                while client.send_tx_blob(&dummy_tx).await.is_ok() {
                    request_count += 1;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                request_count
            }
        });

        // Let it send a few requests.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Drop the node context, which will trigger graceful shutdown
        drop(node);

        // Wait for the request task to complete and get the request count
        let request_count = request_handle.await.expect("Request task failed");

        // Ensure we made at least a few successful requests
        assert!(request_count > 0, "No successful requests were made");

        // Verify server has stopped by attempting a request
        // Create a client using NodeApiHttpClient
        let err = client
            .get_node_info()
            .await
            .expect_err("Expected request to fail after shutdown");
        assert!(
            err.to_string().contains("etting node info request fai"),
            "Expected connection error, got: {}",
            err
        );
    }
}
