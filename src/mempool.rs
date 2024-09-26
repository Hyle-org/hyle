//! Mempool logic & pending transaction management.

use crate::{
    bus::{bus_client, command_response::Query, BusMessage, SharedMessageBus},
    consensus::{ConsensusCommand, ConsensusEvent},
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
use std::collections::HashSet;
use tracing::{debug, info, warn};

mod metrics;

bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(ConsensusCommand),
    receiver(Query<MempoolCommand, MempoolResponse>),
    receiver(SignedWithId<MempoolNetMessage>),
    receiver(RestApiMessage),
    receiver(ConsensusEvent),
    receiver(ValidatorRegistryNetMessage),
}
}

pub struct Mempool {
    crypto: SharedBlstCrypto,
    metrics: MempoolMetrics,
    validators: ValidatorRegistry,
    // txs accumulated, not yet transmitted to the consensus
    pending_txs: Vec<Transaction>,
    batched_txs: HashSet<Vec<Transaction>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
}
impl BusMessage for MempoolNetMessage {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MempoolCommand {
    CreatePendingBatch,
}
impl BusMessage for MempoolCommand {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MempoolResponse {
    PendingBatch { txs: Vec<Transaction> },
}
impl BusMessage for MempoolResponse {}

impl Module for Mempool {
    type Context = SharedRunContext;

    fn name() -> &'static str {
        "Mempool"
    }

    async fn build(ctx: &SharedRunContext) -> Result<Self> {
        Ok(Mempool::new(ctx.config.clone(), ctx.crypto.clone()).await)
    }

    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        self.start(ctx.bus.new_handle())
    }
}

impl Mempool {
    pub async fn new(config: SharedConf, crypto: SharedBlstCrypto) -> Mempool {
        Mempool {
            metrics: MempoolMetrics::global(&config),
            crypto,
            validators: ValidatorRegistry::default(),
            pending_txs: vec![],
            batched_txs: HashSet::default(),
        }
    }

    /// start starts the mempool server.
    pub async fn start(&mut self, shared_bus: SharedMessageBus) -> Result<()> {
        info!("Mempool starting");

        let mut bus = MempoolBusClient::new_from_bus(shared_bus.new_handle()).await;
        handle_messages! {
            on_bus bus,
            command_response<MempoolCommand, MempoolResponse> cmd => {
                 self.handle_command(cmd)
            }
            listen<SignedWithId<MempoolNetMessage>> cmd => {
                self.handle_net_message(cmd, &bus).await
            }
            listen<RestApiMessage> cmd => {
                self.handle_api_message(cmd, &bus).await
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
            ConsensusEvent::CommitBlock { block } => {
                // Remove all txs that have been commited
                self.pending_txs.retain(|tx| !block.txs.contains(tx));
            }
        }
    }

    async fn handle_net_message(
        &mut self,
        msg: SignedWithId<MempoolNetMessage>,
        bus: &MempoolBusClient,
    ) {
        match self.validators.check_signed(&msg) {
            Ok(valid) => {
                if valid {
                    match msg.msg {
                        MempoolNetMessage::NewTx(tx) => self.on_new_tx(tx, bus).await,
                    }
                } else {
                    warn!("Invalid signature for message {:?}", msg);
                }
            }
            Err(e) => warn!("Error while checking signed message: {}", e),
        }
    }

    async fn handle_api_message(&mut self, command: RestApiMessage, bus: &MempoolBusClient) {
        match command {
            RestApiMessage::NewTx(tx) => {
                self.on_new_tx(tx.clone(), bus).await;
                self.broadcast_tx(tx, bus).await.ok();
            }
        }
    }

    async fn on_new_tx(&mut self, tx: Transaction, bus: &MempoolBusClient) {
        debug!("Got new tx {} {:?}", tx.hash(), tx);
        self.metrics.add_api_tx("blob".to_string());
        self.pending_txs.push(tx.clone());
        // TODO: Change this when we implement Motorway
        if self.pending_txs.len() == 1 {
            let batch = self.pending_txs.drain(0..).collect::<Vec<Transaction>>();
            self.batched_txs.insert(batch.clone());
            _ = bus
                .send(ConsensusCommand::ReceiveLatestBatch(batch))
                .context("Cannot send message over channel")
                .ok();
        }
        self.metrics.snapshot_pending_tx(self.pending_txs.len());
    }

    async fn broadcast_tx(&mut self, tx: Transaction, bus: &MempoolBusClient) -> Result<()> {
        self.metrics.add_broadcasted_tx("blob".to_string());
        _ = bus
            .send(OutboundMessage::broadcast(
                self.sign_net_message(MempoolNetMessage::NewTx(tx))?,
            ))
            .context("broadcasting tx")?;
        Ok(())
    }

    fn sign_net_message(&self, msg: MempoolNetMessage) -> Result<SignedWithId<MempoolNetMessage>> {
        self.crypto.sign(msg)
    }

    fn handle_command(&mut self, command: &mut MempoolCommand) -> Result<MempoolResponse> {
        match command {
            // TODO remove
            MempoolCommand::CreatePendingBatch => {
                info!("Creating pending transaction batch");
                let batch: Vec<Transaction> = self.pending_txs.iter().take(2).cloned().collect();
                self.metrics.snapshot_batched_tx(batch.len());
                self.metrics.add_batch();
                Ok(MempoolResponse::PendingBatch { txs: batch })
            }
        }
    }
}
