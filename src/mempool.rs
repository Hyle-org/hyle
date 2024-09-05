use crate::logger::LogMe;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    select,
    sync::mpsc::{unbounded_channel, UnboundedReceiver},
};
use tracing::{error, info};

use crate::model::Transaction;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Peer(usize, String);

#[derive(Debug)]
pub struct Mempool {
    id: usize,
    peers: Vec<Peer>,
    txs: Vec<Transaction>,
    // db
}

impl Mempool {
    pub fn new(id: usize, peers: Vec<Peer>) -> Mempool {
        Mempool {
            id,
            txs: vec![],
            peers,
        }
    }

    /// new_tx is called by the RPC server when a new transaction is sent.
    pub async fn new_tx(&mut self, tx: Transaction) -> Result<()> {
        let r = self.broadcast_tx(&tx).await;
        info!("{}: mempool tx: {}", self.id, tx.inner);
        info!("{}: has {} txs", self.id, self.txs.len());
        self.txs.push(tx);
        r
    }

    /// broadcast_tx is called to replicate the transaction to the other nodes.
    async fn broadcast_tx(&self, tx: &Transaction) -> Result<()> {
        for peer in self.peers.iter() {
            if peer.0 != self.id {
                if let Ok(mut s) = TcpStream::connect(&peer.1)
                    .await
                    .log_error(format!("connecting to node {}", peer.1))
                {
                    _ = s
                        .write_all(tx.as_bytes())
                        .await
                        .log_error("broadcasting tx");
                }
            }
        }
        Ok(())
    }

    /// start starts the mempool server.
    pub async fn start(&mut self, mut rpc_receiver: UnboundedReceiver<Transaction>) -> Result<()> {
        let addr = self
            .peers
            .iter()
            .find(|Peer(id, _)| *id == self.id)
            .expect("invalid mempool_peers config or wrong id")
            .1
            .to_owned();
        let (sender, mut receiver) = unbounded_channel::<String>();
        tokio::spawn(async move {
            if let Ok(listener) = TcpListener::bind(&addr)
                .await
                .log_error("binding mempool socket")
            {
                info!("mempool listening on {}", addr);
                let mut buf = Vec::with_capacity(1024);
                loop {
                    if let Ok((mut s, _)) = listener
                        .accept()
                        .await
                        .log_error("accepting mempool client")
                    {
                        if let Ok(n) = s.read_u64().await {
                            if buf.len() < n as usize {
                                buf.resize(n as usize, 0);
                            }
                            if let Ok(_) = s
                                .read_exact(buf.as_mut_slice())
                                .await
                                .log_error("reading message")
                            {
                                let str = String::from_utf8_lossy(&buf);
                                _ = sender
                                    .send(str.to_string())
                                    .log_error("sending message to mempool");
                            }
                        }
                    }
                }
            }
            panic!("starting mempool server")
        });
        loop {
            select! {
                    Some(_sync_req) = receiver.recv() => {
                        todo!()
                    },
                    Some(transaction) = rpc_receiver.recv() => {
                        if let Err(e) = self.new_tx(transaction).await {
                            error!("broadcasting transaction: {:#}", e);
                        }
                    },
            }
        }
    }
}
