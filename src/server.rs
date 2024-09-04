use crate::ctx::Ctx;
use crate::model::Block;
use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tracing::info;

pub async fn server(addr: &str) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("listening on {}", addr);

    loop {
        let (mut socket, _) = listener.accept().await?;

        tokio::spawn(async move {
            let mut buf = vec![0; 1024];
            let mut ctx = Ctx::default();
            let genesis = Block::genesis();
            ctx.add_block(genesis);

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
