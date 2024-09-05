use anyhow::{Context, Result};
use rand::{distributions::Alphanumeric, Rng};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::Duration;

use crate::model::Transaction;
use crate::p2p_network::NetMessage;

pub fn new_transaction() -> Vec<u8> {
    NetMessage::NewTransaction(Transaction {
        inner: rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(7)
            .map(char::from)
            .collect(),
    })
    .as_binary()
}

pub async fn client(addr: &str) -> Result<()> {
    let mut socket = TcpStream::connect(&addr)
        .await
        .context("connecting to server")?;
    loop {
        socket
            .write(new_transaction().as_ref())
            .await
            .context("sending message")?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
