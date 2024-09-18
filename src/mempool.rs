use crate::{
    bus::{command_response::NeedAnswer, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{Hashable, Transaction},
<<<<<<< HEAD
    p2p::network::{MempoolNetMessage, OutboundMessage, Signed, SignedMempoolNetMessage},
=======
    p2p::network::{MempoolNetMessage, OutboundMessage, ReplicaRegistryNetMessage, Signed},
    replica_registry::ReplicaRegistry,
>>>>>>> a8e9709 (✨ Add ReplicaRegistry)
    rest::endpoints::RestApiMessage,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

#[allow(dead_code)]
#[derive(Debug)]
struct Batch(String, Vec<Transaction>);

pub struct Mempool {
    bus: SharedMessageBus,
    replicas: ReplicaRegistry,
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
            replicas: ReplicaRegistry::default(),
            pending_txs: vec![],
            pending_batches: HashMap::new(),
            committed_batches: vec![],
        }
    }

    /// start starts the mempool server.
    pub async fn start(&mut self) {
        impl NeedAnswer<MempoolResponse> for MempoolCommand {}
<<<<<<< HEAD
        handle_messages! {
            command_response<MempoolCommand, MempoolResponse>(self.bus) = cmd => {
                 self.handle_command(cmd)
            },
            listen<SignedMempoolNetMessage>(self.bus) = cmd => {
                self.handle_net_message(cmd).await
            },
            listen<RestApiMessage>(self.bus) = cmd => {
                self.handle_api_message(cmd).await
            },
            listen<ConsensusEvent>(self.bus) = cmd => {
                self.handle_event(cmd);
            },
=======
        let mut mempool_server = self
            .bus
            .create_server::<MempoolCommand, MempoolResponse>()
            .await;
        let mut net_receiver = self.bus.receiver::<Signed<MempoolNetMessage>>().await;
        let mut api_receiver = self.bus.receiver::<RestApiMessage>().await;
        let mut event_receiver = self.bus.receiver::<ConsensusEvent>().await;
        let mut replica_registry_receiver = self.bus.receiver::<ReplicaRegistryNetMessage>().await;

        loop {
            select! {
                Ok(cmd) = net_receiver.recv() => {
                    self.handle_net_message(cmd).await
                }
                Ok(cmd) = api_receiver.recv() => {
                    self.handle_api_message(cmd).await
                }
                Ok(query) = mempool_server.get_query() => {
                    handle_server_query!(mempool_server, query, self, handle_command);
                }
                Ok(cmd) = event_receiver.recv() => {
                    self.handle_event(cmd);
                }
                Ok(cmd) = replica_registry_receiver.recv() => {
                    self.replicas.handle_net_message(cmd).await;
                }
            }
>>>>>>> a8e9709 (✨ Add ReplicaRegistry)
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

<<<<<<< HEAD
    async fn handle_net_message(&mut self, command: Signed<MempoolNetMessage>) {
        match command.msg {
            MempoolNetMessage::NewTx(tx) => self.on_new_tx(tx).await,
=======
    async fn handle_net_message(&mut self, msg: Signed<MempoolNetMessage>) {
        match self.replicas.check_signed(&msg) {
            Ok(valid) => {
                if valid {
                    match msg.msg {
                        MempoolNetMessage::NewTx(tx) => self.on_new_tx(tx, false).await,
                    }
                } else {
                    warn!("Invalid signature for message {:?}", msg);
                }
            }
            Err(e) => warn!("Error while checking signed message: {}", e),
>>>>>>> a8e9709 (✨ Add ReplicaRegistry)
        }
    }

    async fn handle_api_message(&mut self, command: RestApiMessage) {
        match command {
            RestApiMessage::NewTx(tx) => {
                self.on_new_tx(tx.clone()).await;
                self.broadcast_tx(tx).await
            }
        }
    }

    async fn on_new_tx(&mut self, tx: Transaction) {
        info!("Got new tx {} {:?}", tx.hash(), tx);
        self.pending_txs.push(tx);
    }

    async fn broadcast_tx(&mut self, tx: Transaction) {
        self.bus
            .sender::<OutboundMessage>()
            .await
            .send(OutboundMessage::broadcast(
                self.sign_net_message(MempoolNetMessage::NewTx(tx)),
            ))
            .map(|_| ())
            .ok();
    }

    fn sign_net_message(&self, msg: MempoolNetMessage) -> Signed<MempoolNetMessage> {
        Signed {
            msg,
            signature: Default::default(),
            replica_id: Default::default(),
        }
    }

    fn handle_command(&mut self, command: MempoolCommand) -> Result<Option<MempoolResponse>> {
        match command {
            MempoolCommand::CreatePendingBatch { id } => {
                info!("Creating pending transaction batch with id {}", id);
                let txs: Vec<Transaction> = self.pending_txs.drain(0..).collect();
                self.pending_batches.insert(id.clone(), txs.clone());
                Ok(Some(MempoolResponse::PendingBatch { id, txs }))
            }
        }
    }
}
