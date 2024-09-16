use crate::{
    bus::command_response::{CommandResponseServerCreate, NeedAnswer},
    bus::SharedMessageBus,
    consensus::ConsensusEvent,
    handle_server_query,
    model::Hashable,
    model::Transaction,
    p2p::network::{MempoolNetMessage, NetInput},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::select;
use tracing::{info, warn};

#[derive(Debug)]
struct Batch(String, Vec<Transaction>);

pub struct Mempool {
    bus: SharedMessageBus,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MempoolResponse {
    PendingBatch { id: String, txs: Vec<Transaction> },
}

impl Mempool {
    pub fn new(bus: SharedMessageBus) -> Mempool {
        Mempool {
            bus,
            pending_txs: vec![],
            pending_batches: HashMap::new(),
            committed_batches: vec![],
        }
    }

    /// start starts the mempool server.
    pub async fn start(&mut self) {
        impl NeedAnswer<MempoolResponse> for MempoolCommand {}
        let mut mempool_server = self
            .bus
            .create_server::<MempoolCommand, MempoolResponse>()
            .await;
        let mut net_receiver = self.bus.receiver::<NetInput<MempoolNetMessage>>().await;
        let mut event_receiver = self.bus.receiver::<ConsensusEvent>().await;
        loop {
            select! {
                Ok(cmd) = net_receiver.recv() => {
                    self.handle_net_input(cmd)
                }
                Ok(query) = mempool_server.get_query() => {
                    handle_server_query!(mempool_server, query, self, handle_command);
                }
                Ok(cmd) = event_receiver.recv() => {
                    self.handle_event(cmd);
                }
            }
        }
    }

    fn handle_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitBlock { batch_id, block: _ } => {
                // TODO: do we really need a Batch id?
                match self.pending_batches.remove(&batch_id) {
                    Some(pb) => {
                        info!("Committing transactions batch {}", &batch_id);
                        self.committed_batches.push(Batch(batch_id.clone(), pb));
                    }
                    None => {
                        warn!("Tried to commit an unknown pending batch {}", batch_id);
                    }
                }
            }
        }
    }

    fn handle_net_input(&mut self, command: NetInput<MempoolNetMessage>) {
        match command.msg {
            MempoolNetMessage::NewTx(tx) => {
                info!("Got new tx {} {:?}", tx.hash(), tx);
                self.pending_txs.push(tx);
            }
        }
    }
    fn handle_command(&mut self, command: MempoolCommand) -> Result<Option<MempoolResponse>> {
        match command {
            MempoolCommand::CreatePendingBatch { id } => {
                info!("Creating pending transaction batch with id {}", id);
                let txs: Vec<Transaction> = self.pending_txs.drain(0..).collect();
                self.pending_batches.insert(id.clone(), txs.clone());
                return Ok(Some(MempoolResponse::PendingBatch { id, txs }));
            }
        }
    }
}
