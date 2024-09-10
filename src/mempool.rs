use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};
use tracing::{info, warn};

use crate::{model::Transaction, p2p::network::MempoolMessage, utils::logger::LogMe};

#[derive(Debug)]
struct Batch(String, Vec<Transaction>);

#[derive(Debug)]
pub struct Mempool {
    // txs accumulated, not yet transmitted to the consensus
    pending_txs: Vec<Transaction>,
    // txs batched under a req_id, transmitted to the consensus to be packed in a block
    pending_batches: HashMap<String, Vec<Transaction>>,
    // Block has been committed by the consensus, could be removed at some point
    committed_batches: Vec<Batch>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum MempoolCommand {
    CreatePendingBatch { id: String },
    CommitBatches { ids: Vec<String> },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum MempoolResponse {
    PendingBatch { id: String, txs: Vec<Transaction> },
}

impl Mempool {
    pub fn new() -> Mempool {
        Mempool {
            pending_txs: vec![],
            pending_batches: HashMap::new(),
            committed_batches: vec![],
        }
    }

    /// start starts the mempool server.
    pub async fn start(
        &mut self,
        mut message_receiver: UnboundedReceiver<MempoolMessage>,
        mut commands_receiver: UnboundedReceiver<MempoolCommand>,
        response_sender: UnboundedSender<MempoolResponse>,
    ) -> Result<()> {
        loop {
            select! {
                Some(msg) = message_receiver.recv() => {
                    match msg {
                        MempoolMessage::NewTx(msg) => {
                            self.pending_txs.push(msg);
                        }
                    }
                }

                Some(msg) = commands_receiver.recv() => {
                    _ = self.handle_command(msg, &response_sender);
                }
            }
        }
    }

    fn handle_command(
        &mut self,
        command: MempoolCommand,
        response_sender: &UnboundedSender<MempoolResponse>,
    ) -> Result<()> {
        match command {
            MempoolCommand::CreatePendingBatch { id } => {
                info!("Creating pending transaction batch with id {}", id);
                let txs: Vec<Transaction> = self.pending_txs.drain(0..).collect();
                self.pending_batches.insert(id.clone(), txs.clone());
                _ = response_sender
                    .send(MempoolResponse::PendingBatch {
                        id: id.clone(),
                        txs: txs.clone(),
                    })
                    .log_error("Sending pending batch");
            }
            MempoolCommand::CommitBatches { ids } => {
                for b_id in ids.into_iter() {
                    match self.pending_batches.remove(&b_id) {
                        Some(pb) => {
                            info!("Committing transactions batch {}", &b_id);
                            self.committed_batches.push(Batch(b_id, pb));
                        }
                        None => {
                            warn!("Tried to commit an unknown pending batch {}", b_id);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
