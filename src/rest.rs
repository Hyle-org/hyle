use crate::{indexer::Indexer, utils::conf::SharedConf};
use anyhow::{Context, Result};
use axum::{routing::get, Router};
use tracing::info;

mod endpoints;
pub mod model;

pub async fn rest_server(config: SharedConf, idxr: Indexer) -> Result<()> {
    info!("rest listening on {}", config.rest_addr());
    let app = Router::new()
        .route("/getTransaction/:tx_hash", get(endpoints::get_transaction))
        .route("/getBlock/:height", get(endpoints::get_block))
        .with_state(idxr);

    let listener = tokio::net::TcpListener::bind(config.rest_addr())
        .await
        .context("Starting rest server")?;

    axum::serve(listener, app)
        .await
        .context("Starting rest server")
}
