use anyhow::{Context, Result};
use axum::{routing::get, Router};
use tracing::info;

use crate::utils::conf::SharedConf;

mod endpoints;
pub mod model;

pub async fn rest_server(config: SharedConf) -> Result<()> {
    info!("rest listening on {}", config.rest_addr());
    let app = Router::new()
        .route("/getTransaction", get(endpoints::get_transaction))
        .route("/getBlock", get(endpoints::get_block));

    let listener = tokio::net::TcpListener::bind(config.rest_addr())
        .await
        .context("Starting rest server")?;

    axum::serve(listener, app)
        .await
        .context("Starting rest server")
}
