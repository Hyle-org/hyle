use anyhow::{Context, Result};
use clap::Parser;
use hyle::{
    model::{Transaction, TransactionData},
    p2p::network::NetMessage,
    utils::conf::{self, SharedConf},
};
use rand::{distributions::Alphanumeric, Rng};
use tokio::{io::AsyncWriteExt, net::TcpStream, time::Duration};
use tracing::info;

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
    .to_binary()
}

pub async fn client(config: SharedConf) -> Result<()> {
    let mut socket = TcpStream::connect(config.addr())
        .await
        .context("connecting to server")?;
    info!("Starting client");
    loop {
        info!("Sending a message");
        let res = new_transaction();
        socket
            .write_u32(res.len() as u32)
            .await
            .context("sending tcp message size")?;
        socket
            .write(res.as_ref())
            .await
            .context("sending tcp message")?;
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

    let config = conf::Conf::new_shared(args.config_file)?;
    info!("Starting client with config: {:?}", config);

    info!("client mode");
    client(config).await
}
