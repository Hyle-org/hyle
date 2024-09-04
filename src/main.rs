use model::{Block, Transaction};
use rand::{distributions::Alphanumeric, Rng};
use tokio::time::{Duration, Instant};

use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::info;

mod model;

use anyhow::{Context, Result};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    client: Option<bool>,
}

fn new_transaction() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(7)
        .map(char::from)
        .collect()
}

#[derive(Default)]
struct Ctx {
    mempool: Vec<Transaction>,
    blocks: Vec<Block>,
}
impl Ctx {
    fn handle_tx(&mut self, data: &str) {
        self.mempool.push(Transaction {
            inner: data.to_string(),
        });

        let last_block = self.blocks.last().unwrap();
        let last = last_block.timestamp;

        if Instant::now() - last > Duration::from_secs(5) {
            self.blocks.push(Block {
                parent_hash: last_block.hash_block(),
                height: last_block.height + 1,
                timestamp: Instant::now(),
                txs: self.mempool.drain(0..).collect(),
            });
            info!("New block {:?}", self.blocks.last());
        } else {
            info!("New tx: {}", data);
        }
    }
}

async fn client(addr: &str) -> Result<()> {
    let mut socket = TcpStream::connect(&addr)
        .await
        .context("connecting to server")?;
    loop {
        socket
            .write(format!("{}", new_transaction()).as_ref())
            .await
            .context("sending message")?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn server(addr: &str) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!("listening on {}", addr);

    loop {
        let (mut socket, _) = listener.accept().await?;

        tokio::spawn(async move {
            let mut buf = vec![0; 1024];
            let mut ctx = Ctx::default();
            let genesis = Block::genesis();
            ctx.blocks.push(genesis);

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
