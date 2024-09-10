use anyhow::{Context, Result};
use rand::{distributions::Alphanumeric, Rng};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::Duration;

use clap::Parser;
use hyle::model::{Transaction, TransactionData};
use hyle::p2p::network::NetMessage;
use tracing::{debug, info};

use hyle::utils::conf;

pub fn new_transaction() -> Vec<u8> {
    NetMessage::NewTransaction(Transaction {
        inner: rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(7)
            .map(char::from)
            .collect(),
        version: 1,
        transaction_data: TransactionData::default(),
    })
    .as_binary()
}

pub async fn client(addr: &str) -> Result<()> {
    let mut socket = TcpStream::connect(&addr)
        .await
        .context("connecting to server")?;
    info!("Starting client");
    loop {
        info!("Sending a message");
        let res = new_transaction();
        socket
            .write_u32(res.len() as u32)
            .await
            .context("sending message")?;

        socket
            .write(res.as_ref())
            .await
            .context("sending message")?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "master.ron")]
    config_file: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let config = conf::Conf::new(args.config_file)?;
    info!("Starting client with config: {:?}", config);

    let rpc_addr = config.addr();

    info!("client mode");
    client(&rpc_addr).await?;

    Ok(())
}
