//! Mempool logic & pending transaction management.

use crate::{
    bus::{bus_client, command_response::Query, BusMessage, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{Hashable, SharedRunContext, Transaction, TransactionData},
    p2p::network::{OutboundMessage, SignedWithKey},
    rest::endpoints::RestApiMessage,
    utils::{
        crypto::{BlstCrypto, SharedBlstCrypto},
        modules::Module,
    },
};
use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use metrics::MempoolMetrics;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};
use tracing::{debug, info, warn};

mod metrics;

bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(MempoolEvent),
    receiver(Query<MempoolCommand, MempoolResponse>),
    receiver(SignedWithKey<MempoolNetMessage>),
    receiver(RestApiMessage),
    receiver(ConsensusEvent),
}
}

pub struct Mempool {
    bus: MempoolBusClient,
    crypto: SharedBlstCrypto,
    metrics: MempoolMetrics,
    // txs accumulated, not yet transmitted to the consensus
    pending_txs: Vec<Transaction>,
    batched_txs: HashSet<Vec<Transaction>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
}
impl BusMessage for MempoolNetMessage {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MempoolCommand {
    CreatePendingBatch,
}
impl BusMessage for MempoolCommand {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum MempoolEvent {
    LatestBatch(Vec<Transaction>),
}
impl BusMessage for MempoolEvent {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MempoolResponse {
    PendingBatch { txs: Vec<Transaction> },
}
impl BusMessage for MempoolResponse {}

impl Module for Mempool {
    fn name() -> &'static str {
        "Mempool"
    }

    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = MempoolBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        let metrics = MempoolMetrics::global(ctx.common.config.id.clone());
        Ok(Mempool {
            bus,
            metrics,
            crypto: Arc::clone(&ctx.node.crypto),
            pending_txs: vec![],
            batched_txs: HashSet::default(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl Mempool {
    /// start starts the mempool server.
    pub async fn start(&mut self) -> Result<()> {
        info!("Mempool starting");

        handle_messages! {
            on_bus self.bus,
            command_response<MempoolCommand, MempoolResponse> cmd => {
                 self.handle_command(cmd)
            }
            listen<SignedWithKey<MempoolNetMessage>> cmd => {
                self.handle_net_message(cmd).await
            }
            listen<RestApiMessage> cmd => {
                self.handle_api_message(cmd).await
            }
            listen<ConsensusEvent> cmd => {
                self.handle_event(cmd);
            }
            //listen<ValidatorRegistryNetMessage> cmd => {
            //    self.validators.handle_net_message(cmd);
            //}
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

    async fn handle_net_message(&mut self, msg: SignedWithKey<MempoolNetMessage>) {
        match BlstCrypto::verify(&msg) {
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
                // hacky stuff waiting for staking contract: do not broadcast stake txs
                if !matches!(tx.transaction_data, TransactionData::Stake(_)) {
                    self.broadcast_tx(tx).await.ok();
                }
            }
        }
    }

    async fn on_new_tx(&mut self, tx: Transaction) {
        debug!("Got new tx {}", tx.hash());
        self.metrics.add_api_tx("blob".to_string());
        self.pending_txs.push(tx.clone());
        // TODO: Change this when we implement Motorway
        if self.pending_txs.len() == 1 {
            let batch = self.pending_txs.drain(0..).collect::<Vec<Transaction>>();
            self.batched_txs.insert(batch.clone());
            _ = self
                .bus
                .send(MempoolEvent::LatestBatch(batch))
                .context("Cannot send message over channel")
                .ok();
        }
        self.metrics.snapshot_pending_tx(self.pending_txs.len());
    }

    async fn broadcast_tx(&mut self, tx: Transaction) -> Result<()> {
        self.metrics.add_broadcasted_tx("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::broadcast(
                self.sign_net_message(MempoolNetMessage::NewTx(tx))?,
            ))
            .context("broadcasting tx")?;
        Ok(())
    }

    fn sign_net_message(&self, msg: MempoolNetMessage) -> Result<SignedWithKey<MempoolNetMessage>> {
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
