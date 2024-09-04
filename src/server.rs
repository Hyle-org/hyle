use crate::ctx::Ctx;
use crate::model::Transaction;
use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::info;

pub async fn server(addr: &str) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("listening on {}", addr);

    let (tx, rx) = mpsc::channel::<Transaction>(100);

    tokio::spawn(async move {
        let mut ctx = Ctx::load_from_disk();
        ctx.start(rx).await
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
                tx2.send(Transaction {
                    inner: d.to_string(),
                })
                .await
                .unwrap();
            }
        });
    }
}
