use crate::{
    bus::SharedMessageBus, model::Transaction, p2p::network::MempoolNetMessage,
    utils::logger::LogMe,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::{select, sync::broadcast::Sender};
use tracing::{info, warn};

#[derive(Debug)]
struct Batch(String, Vec<Transaction>);

pub struct Mempool {
    bus: SharedMessageBus,
    net_bus: SharedMessageBus,
    // txs accumulated, not yet transmitted to the consensus
    pending_txs: Vec<Transaction>,
    // txs batched under a req_id, transmitted to the consensus to be packed in a block
    pending_batches: HashMap<String, Vec<Transaction>>,
    // Block has been committed by the consensus, could be removed at some point
    committed_batches: Vec<Batch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MempoolCommand {
    CreatePendingBatch { id: String },
    CommitBatches { ids: Vec<String> },
    HandleNetMessage(MempoolNetMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MempoolResponse {
    PendingBatch { id: String, txs: Vec<Transaction> },
}

impl Mempool {
    pub fn new(bus: SharedMessageBus, net_bus: SharedMessageBus) -> Mempool {
        Mempool {
            bus,
            net_bus,
            pending_txs: vec![],
            pending_batches: HashMap::new(),
            committed_batches: vec![],
        }
    }

    /// start starts the mempool server.
    pub async fn start(&mut self) {
        let mut command_receiver = self.bus.receiver::<MempoolCommand>().await;
        let mut net_receiver = self.net_bus.receiver::<MempoolCommand>().await;
        let response_sender = self.bus.sender::<MempoolResponse>().await;
        loop {
            select! {
                Ok(cmd) = net_receiver.recv() => {
                    _ = self.handle_command(cmd, &response_sender)
                }
                Ok(cmd) = command_receiver.recv() => {
                    _ = self.handle_command(cmd, &response_sender);
                }
            }
        }
    }

    fn handle_command(
        &mut self,
        command: MempoolCommand,
        response_sender: &Sender<MempoolResponse>,
    ) -> Result<()> {
        match command {
            MempoolCommand::CreatePendingBatch { id } => {
                info!("Creating pending transaction batch with id {}", id);
                let txs: Vec<Transaction> = self.pending_txs.drain(0..).collect();
                self.pending_batches.insert(id.clone(), txs.clone());
                _ = response_sender
                    .send(MempoolResponse::PendingBatch { id, txs })
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
            MempoolCommand::HandleNetMessage(msg) => match msg {
                MempoolNetMessage::NewTx(tx) => {
                    self.pending_txs.push(tx);
                }
            },
        }
        Ok(())
    }
}
