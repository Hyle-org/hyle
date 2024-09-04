use std::time::Duration;

use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::info;

use anyhow::{Context, Result};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    client: Option<bool>,
}

async fn client(addr: &str) -> Result<()> {
    let mut i = 0;
    let mut socket = TcpStream::connect(&addr)
        .await
        .context("connecting to server")?;
    loop {
        socket
            .write(format!("id {}", i).as_ref())
            .await
            .context("sending message")?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        i += 1;
    }
}

async fn server(addr: &str) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("listening on {}", addr);

    loop {
        let (mut socket, _) = listener.accept().await?;

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
                info!("{}", std::str::from_utf8(&buf[0..n]).unwrap());
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let addr = "127.0.0.1:1234";

    if args.client.unwrap_or(false) {
        info!("client mode");
        client(&addr).await?;
    }

    info!("server mode");
    return server(&addr).await;
}
