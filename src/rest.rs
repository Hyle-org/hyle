use anyhow::{Context, Result};
use axum::routing::get;
use axum::Router;
use tracing::info;

mod endpoints;
pub mod model;

pub async fn rest_server(addr: &str) -> Result<()> {
    info!("rest listening on {}", addr);
    let app = Router::new()
        .route("/getTransaction", get(endpoints::get_transaction))
        .route("/getBlock", get(endpoints::get_block));

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("Starting rest server")?;

    axum::serve(listener, app)
        .await
        .context("Starting rest server")
}
