use crate::conf::Conf;
use crate::ctx::{Ctx, CtxCommand};
use crate::model::Transaction;
use crate::p2p::peer;
use crate::rest_endpoints;
use anyhow::{Context, Error, Result};
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

pub fn run_as_master(
    tx: mpsc::Sender<CtxCommand>,
    rx: mpsc::Receiver<CtxCommand>,
    config: &Conf,
    sender: UnboundedSender<Transaction>,
) {
    tokio::spawn(async move {
        let mut ctx = Ctx::load_from_disk().unwrap_or_else(|_| {
            warn!("Failed to load ctx from disk, using a default one");
            Ctx::default()
        });

        _ = ctx.start(rx, sender).await;
    });

    let interval = config.storage.interval;

    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(interval)).await;

            tx.send(CtxCommand::GenerateNewBlock)
                .await
                .expect("Cannot send message over channel");
            tx.send(CtxCommand::SaveOnDisk)
                .await
                .expect("Cannot send message over channel");
        }
    });
}

pub async fn p2p_server(
    addr: &str,
    config: &Conf,
    sender: UnboundedSender<Transaction>,
) -> Result<(), Error> {
    let (tx, rx) = mpsc::channel::<CtxCommand>(100);

    if config.peers.is_empty() {
        warn!("No peers in conf, running as master");
        run_as_master(tx.clone(), rx, config, sender);

        let listener = TcpListener::bind(addr).await?;
        info!("p2p listening on {}", addr);

        loop {
            let (socket, _) = listener.accept().await?;
            let tx2 = tx.clone();

            tokio::spawn(async move {
                info!(
                    "New peer: {}",
                    socket
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or("no address".to_string())
                );
                let mut peer_server = peer::Peer::new(socket, tx2.clone()).await?;
                match peer_server.start().await {
                    Ok(_) => info!("Peer thread exited"),
                    Err(e) => info!("Peer thread exited: {}", e),
                }
                anyhow::Ok(())
            });
        }
    } else {
        let peer_address = config.peers.first().unwrap();
        info!("Connecting to peer {}", peer_address);
        let stream = peer::Peer::connect(peer_address).await?;
        let mut peer = peer::Peer::new(stream, tx.clone()).await?;

        peer.handshake().await?;
        peer.start().await
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
