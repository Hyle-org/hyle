use anyhow::{Context, Ok, Result};
use axum::routing::get;
use axum::Router;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tracing::info;

use crate::ctx::Ctx;
use crate::model::Block;
use crate::rest_endpoints;

pub async fn rpc_server(addr: &str) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("rpc listening on {}", addr);

    loop {
        let (mut socket, _) = listener.accept().await?;

        tokio::spawn(async move {
            let mut buf = vec![0; 1024];
            let mut ctx = Ctx::default();
            ctx.add_block(Block::default());

            loop {
                let n = socket
                    .read(&mut buf)
                    .await
                    .context("reading from socket")
                    .unwrap();
                if n == 0 {
                    info!("houston ?");
                    return;
                }
                let d = std::str::from_utf8(&buf[0..n]).unwrap();
                ctx.handle_tx(d);
            }
        });
    }
}

pub async fn rest_server(addr: &str) -> Result<()> {
    info!("rest listening on {}", addr);
    let app = Router::new()
        .route("/getTransaction", get(rest_endpoints::get_transaction))
        .route("/getBlock", get(rest_endpoints::get_block));

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
    return Ok(());
}
