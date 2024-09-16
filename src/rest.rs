use crate::{bus::SharedMessageBus, indexer::Indexer, utils::conf::SharedConf};
use anyhow::{Context, Result};
use axum::{
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
