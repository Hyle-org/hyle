use crate::{
    bus::{
        command_response::{CommandResponseServer, CommandResponseServerCreate, NeedAnswer},
        listener::Listener,
        SharedMessageBus,
    },
    mempool,
    model::Transaction,
    p2p::network::{MempoolNetMessage, NetInput},
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MempoolResponse {
    PendingBatch { id: String, txs: Vec<Transaction> },
}

macro_rules! handle_query {
    ($server:expr, $query:expr, $self:ident, $handler:ident) => {
        let (cmd, response_writer) = $server.to_response($query);
        let res = $self.$handler(cmd);
        let _ = $server.respond(response_writer.updated(res));
    };
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
        let mut mempool_server: CommandResponseServer<MempoolCommand, MempoolResponse> =
            self.bus.create_server().await;
        let mut net_receiver = self.bus.receiver::<NetInput<MempoolNetMessage>>().await;
        loop {
            select! {
                Ok(cmd) = net_receiver.recv() => {
                    self.handle_net_input(cmd)
                }
                Ok(query) = mempool_server.get_query() => {
                    handle_query!(mempool_server, query, self, handle_command);
                }
            }
        }
    }

    fn handle_net_input(&mut self, command: NetInput<MempoolNetMessage>) {
        match command.msg {
            MempoolNetMessage::NewTx(tx) => {
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
        Ok(None)
    }
}
