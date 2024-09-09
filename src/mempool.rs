use anyhow::Result;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::{model::Transaction, p2p::network::MempoolMessage};

#[derive(Debug)]
pub struct Mempool {
    txs: Vec<Transaction>,
    // db
}

impl Mempool {
    pub fn new() -> Mempool {
        Mempool { txs: vec![] }
    }

    /// start starts the mempool server.
    pub async fn start(&mut self, mut receiver: UnboundedReceiver<MempoolMessage>) -> Result<()> {
        loop {
            match receiver.recv().await {
                Some(MempoolMessage::NewTx(msg)) => {
                    self.txs.push(msg);
                }
                None => {}
            }
        }
    }
}
