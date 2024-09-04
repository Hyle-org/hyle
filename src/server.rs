use crate::config::Config;
use crate::ctx::{Ctx, CtxCommand};
use crate::model::Transaction;
use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::info;

pub async fn server(addr: &str, config: &Config) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("listening on {}", addr);

    let (tx, rx) = mpsc::channel::<CtxCommand>(100);

    tokio::spawn(async move {
        let mut ctx = Ctx::load_from_disk();
        ctx.start(rx).await
    });

    let tx1 = tx.clone();
    let interval = config.storage.interval;

    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(interval)).await;

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
                let d = std::str::from_utf8(&buf[0..n]).unwrap();
                tx2.send(CtxCommand::AddTransaction(Transaction {
                    inner: d.to_string(),
                }))
                .await
                .unwrap();
            }
        });
    }
}
