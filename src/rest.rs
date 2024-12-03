//! Public API for interacting with the node.

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use axum_otel_metrics::HttpMetricsLayer;
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::{bus::SharedMessageBus, model::ValidatorPublicKey, utils::modules::Module};

pub mod client;

#[derive(Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub pubkey: Option<ValidatorPublicKey>,
    pub da_address: String,
}

pub struct RestApiRunContext {
    pub rest_addr: String,
    pub info: NodeInfo,
    pub bus: SharedMessageBus,
    pub router: Router,
    pub metrics_layer: HttpMetricsLayer,
}

pub struct RouterState {
    info: NodeInfo,
}

pub struct RestApi {
    rest_addr: String,
    app: Option<Router>,
}
impl Module for RestApi {
    fn name() -> &'static str {
        "RestApi"
    }

    type Context = RestApiRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let app = ctx
            .router
            .merge(
                Router::new()
                    .route("/v1/info", get(get_info))
                    .with_state(RouterState { info: ctx.info })
                    .nest("/v1", ctx.metrics_layer.routes()),
            )
            .layer(ctx.metrics_layer)
            .layer(tower_http::cors::CorsLayer::permissive())
            // TODO: Tracelayer should be added only in "dev mode"
            .layer(TraceLayer::new_for_http());
        Ok(RestApi {
            rest_addr: ctx.rest_addr.clone(),
            app: Some(app),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.serve()
    }
}

pub async fn get_info(State(state): State<RouterState>) -> Result<impl IntoResponse, AppError> {
    Ok(Json(state.info))
}

impl RestApi {
    pub async fn serve(&mut self) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(&self.rest_addr)
            .await
            .context("Starting rest server")?;

        info!("rest listening on {}", self.rest_addr);

        axum::serve(listener, self.app.take().expect("app is not set"))
            .await
            .context("Starting rest server")
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
