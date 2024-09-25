//! Mempool logic & pending transaction management.

use crate::{
    bus::{command_response::NeedAnswer, BusMessage, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{Hashable, SharedRunContext, Transaction},
    p2p::network::{OutboundMessage, SignedWithId},
    rest::endpoints::RestApiMessage,
    utils::{conf::SharedConf, crypto::SharedBlstCrypto, modules::Module},
    validator_registry::{ValidatorRegistry, ValidatorRegistryNetMessage},
};
use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use metrics::MempoolMetrics;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

mod metrics;

#[allow(dead_code)]
#[derive(Debug)]
struct Batch(String, Vec<Transaction>);

pub struct Mempool {
    bus: SharedMessageBus,
    crypto: SharedBlstCrypto,
    metrics: MempoolMetrics,
    validators: ValidatorRegistry,
    // txs accumulated, not yet transmitted to the consensus
    pending_txs: Vec<Transaction>,
    // txs batched under a req_id, transmitted to the consensus to be packed in a block
    pending_batches: HashMap<String, Vec<Transaction>>,
    // Block has been committed by the consensus, could be removed at some point
    committed_batches: Vec<Batch>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
}
impl BusMessage for MempoolNetMessage {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MempoolCommand {
    CreatePendingBatch { id: String },
}
impl NeedAnswer<MempoolResponse> for MempoolCommand {}
impl BusMessage for MempoolCommand {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MempoolResponse {
    PendingBatch { id: String, txs: Vec<Transaction> },
}
impl BusMessage for MempoolResponse {}

impl Module for Mempool {
    type Context = SharedRunContext;

    fn name() -> &'static str {
        "Mempool"
    }

    fn build(ctx: &SharedRunContext) -> Result<Self> {
        Ok(Mempool::new(
            ctx.bus.new_handle(),
            ctx.config.clone(),
            ctx.crypto.clone(),
        ))
    }

    fn run(&mut self, _ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl Mempool {
    pub fn new(bus: SharedMessageBus, config: SharedConf, crypto: SharedBlstCrypto) -> Mempool {
        Mempool {
            bus,
            metrics: MempoolMetrics::global(&config),
            crypto,
            validators: ValidatorRegistry::default(),
            pending_txs: vec![],
            pending_batches: HashMap::new(),
            committed_batches: vec![],
        }
    }

    /// start starts the mempool server.
    pub async fn start(&mut self) -> Result<()> {
        info!("Mempool starting");
        handle_messages! {
            on_bus self.bus,
            command_response<MempoolCommand, MempoolResponse> cmd => {
                 self.handle_command(cmd)
            }
            listen<SignedWithId<MempoolNetMessage>> cmd => {
                self.handle_net_message(cmd).await
            }
            listen<RestApiMessage> cmd => {
                self.handle_api_message(cmd).await
            }
            listen<ConsensusEvent> cmd => {
                self.handle_event(cmd);
            }
            listen<ValidatorRegistryNetMessage> cmd => {
                self.validators.handle_net_message(cmd);
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

    async fn handle_net_message(&mut self, msg: SignedWithId<MempoolNetMessage>) {
        match self.validators.check_signed(&msg) {
            Ok(valid) => {
                if valid {
                    match msg.msg {
                        MempoolNetMessage::NewTx(tx) => self.on_new_tx(tx).await,
                    }
                } else {
                    warn!("Invalid signature for message {:?}", msg);
                }
            }
            Err(e) => warn!("Error while checking signed message: {}", e),
        }
    }

    async fn handle_api_message(&mut self, command: RestApiMessage) {
        match command {
            RestApiMessage::NewTx(tx) => {
                self.on_new_tx(tx.clone()).await;
                self.broadcast_tx(tx).await.ok();
            }
        }
    }

    async fn on_new_tx(&mut self, tx: Transaction) {
        debug!("Got new tx {} {:?}", tx.hash(), tx);
        self.metrics.add_api_tx("blob".to_string());
        self.pending_txs.push(tx);
        self.metrics.snapshot_pending_tx(self.pending_txs.len());
    }

    async fn broadcast_tx(&mut self, tx: Transaction) -> Result<()> {
        self.metrics.add_broadcasted_tx("blob".to_string());
        self.bus
            .sender::<OutboundMessage>()
            .await
            .send(OutboundMessage::broadcast(
                self.sign_net_message(MempoolNetMessage::NewTx(tx))?,
            ))
            .context("broadcasting tx")?;
        Ok(())
    }

    fn sign_net_message(&self, msg: MempoolNetMessage) -> Result<SignedWithId<MempoolNetMessage>> {
        self.crypto.sign(msg)
    }

    fn handle_command(&mut self, command: MempoolCommand) -> Result<Option<MempoolResponse>> {
        match command {
            MempoolCommand::CreatePendingBatch { id } => {
                info!("Creating pending transaction batch with id {}", id);
                let txs: Vec<Transaction> = self.pending_txs.drain(0..).collect();
                self.pending_batches.insert(id.clone(), txs.clone());
                self.metrics.snapshot_batched_tx(self.pending_batches.len());
                self.metrics.add_batch();
                Ok(Some(MempoolResponse::PendingBatch { id, txs }))
            }
        }
    }
}
