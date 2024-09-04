use crate::conf::Conf;
use crate::ctx::{Ctx, CtxCommand};
use crate::model::Transaction;
use crate::p2p_network::NetMessage;
use crate::rest_endpoints;
use anyhow::{Context, Ok, Result};
use axum::routing::get;
use axum::Router;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

pub async fn rpc_server(addr: &str, config: &Conf) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("rpc listening on {}", addr);

    let (tx, rx) = mpsc::channel::<CtxCommand>(100);

    tokio::spawn(async move {
        let mut ctx = Ctx::load_from_disk().unwrap_or_else(|_| {
            warn!("Failed to load ctx from disk, using a default one");
            Ctx::default()
        });

        ctx.start(rx).await
    });

    let tx1 = tx.clone();
    let interval = config.storage.interval;

    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(interval)).await;

            tx1.send(CtxCommand::GenerateNewBlock)
                .await
                .expect("Cannot send message over channel");
            tx1.send(CtxCommand::SaveOnDisk)
                .await
                .expect("Cannot send message over channel");
        }
    });

    loop {
        let (mut socket, _) = listener.accept().await?;
        let tx2 = tx.clone();

        tokio::spawn(async move {
            let mut buf = vec![0; 1024];

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
                match bincode::deserialize::<NetMessage>(&buf[0..n]) {
                    std::result::Result::Ok(msg) => {
                        msg.handle(&tx2).await;
                    }
                    std::result::Result::Err(_) => todo!(),
                }
            }
        });
    }
}

pub async fn rest_server(addr: &str) -> Result<()> {
    info!("rest listening on {}", addr);
    let app = Router::new()
        .route("/getTransaction", get(rest_endpoints::get_transaction))
        .route("/getBlock", get(rest_endpoints::get_block));

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("Starting rest server")?;

    axum::serve(listener, app)
        .await
        .context("Starting rest server")
}
