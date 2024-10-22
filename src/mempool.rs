//! Mempool logic & pending transaction management.

use crate::{
    bus::{bus_client, command_response::Query, BusMessage, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    mempool::storage::{Car, DataProposal, InMemoryStorage, TipData},
    model::{Hashable, SharedRunContext, Transaction, TransactionData, ValidatorPublicKey},
    p2p::network::{OutboundMessage, SignedWithKey},
    rest::endpoints::RestApiMessage,
    utils::{
        crypto::{BlstCrypto, SharedBlstCrypto},
        logger::LogMe,
        modules::Module,
    },
};
use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use metrics::MempoolMetrics;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};
use storage::ProposalVerdict;
use strum_macros::IntoStaticStr;
use tracing::{debug, error, info, warn};

mod fees_checker;
mod metrics;
mod storage;
pub use storage::{Cut, CutWithTxs};

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
    storage: InMemoryStorage,
    validators: Vec<ValidatorPublicKey>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
    DataProposal(DataProposal),
    DataVote(DataProposal),
    SyncRequest(DataProposal, Option<usize>),
    SyncReply(Vec<Car>),
}
impl BusMessage for MempoolNetMessage {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MempoolCommand {
    CreatePendingBatch,
}
impl BusMessage for MempoolCommand {}

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BatchInfo {
    pub tip: TipData,
    pub validator: ValidatorPublicKey,
}

impl BatchInfo {
    pub fn new(validator: ValidatorPublicKey) -> Self {
        Self {
            tip: TipData::default(),
            validator,
        }
    }
}

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize)]
pub struct Batch {
    pub info: BatchInfo,
    pub txs: Vec<Transaction>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum MempoolEvent {
    NewCut(CutWithTxs),
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
            storage: InMemoryStorage::new(ctx.node.crypto.validator_pubkey().clone()),
            validators: vec![],
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
            listen<SignedWithKey<MempoolNetMessage>> cmd => {
                self.handle_net_message(cmd).await
            }
            listen<RestApiMessage> cmd => {
                self.handle_api_message(cmd).await
            }
            listen<ConsensusEvent> cmd => {
                self.handle_event(cmd);
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                    self.check_data_proposal().await
            }
        }
    }

    fn handle_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitBlock {
                validators,
                cut_lanes,
                ..
            } => {
                self.validators = validators;
                self.storage.update_lanes_after_commit(cut_lanes);
            }
        }
    }

    async fn handle_net_message(&mut self, msg: SignedWithKey<MempoolNetMessage>) {
        match BlstCrypto::verify(&msg) {
            Ok(true) => {
                let validator = msg.validators.first().unwrap();
                match msg.msg {
                    MempoolNetMessage::NewTx(tx) => self.on_new_tx(tx),
                    MempoolNetMessage::DataProposal(data_proposal) => {
                        if let Err(e) = self.on_data_proposal(validator, data_proposal).await {
                            error!("{:?}", e);
                        }
                    }
                    MempoolNetMessage::DataVote(data_proposal) => {
                        self.on_data_vote(validator, data_proposal).await;
                    }
                    MempoolNetMessage::SyncRequest(data_proposal, last_index) => {
                        self.on_sync_request(validator, data_proposal, last_index)
                            .await;
                    }
                    MempoolNetMessage::SyncReply(cars) => {
                        if let Err(e) = self
                            // TODO: we don't know who sent the message
                            .on_sync_reply(validator, cars)
                            .await
                        {
                            error!("{:?}", e);
                        }
                    }
                }
            }
            Ok(false) => {
                self.metrics.signature_error("mempool");
                warn!("Invalid signature for message {:?}", msg);
            }
            Err(e) => error!("Error while checking signed message: {:?}", e),
        }
    }

    async fn handle_api_message(&mut self, command: RestApiMessage) {
        match command {
            RestApiMessage::NewTx(tx) => {
                self.on_new_tx(tx.clone());
                // hacky stuff waiting for staking contract: do not broadcast stake txs
                if !matches!(tx.transaction_data, TransactionData::Stake(_)) {
                    self.broadcast_tx(tx).ok();
                }
            }
        }
    }

    async fn on_sync_reply(
        &mut self,
        validator: &ValidatorPublicKey,
        missing_cars: Vec<Car>,
    ) -> Result<()> {
        info!("{} SyncReply from validator {validator}", self.storage.id);

        debug!(
            "{} adding {} missing cars to lane {validator}",
            self.storage.id,
            missing_cars.len()
        );

        self.storage.add_missing_cars(validator, missing_cars);

        let waiting_proposals = self.storage.get_waiting_proposals(validator);
        for wp in waiting_proposals {
            if let Err(e) = self.on_data_proposal(validator, wp).await {
                error!("{:?}", e);
            }
        }
        Ok(())
    }

    async fn on_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
        last_index: Option<usize>,
    ) {
        info!(
            "{} SyncRequest received from validator {validator} for last_index {:?}",
            self.storage.id, last_index
        );

        let missing_cars = self.storage.get_missing_cars(last_index, &data_proposal);
        debug!("Missing cars on {} are {:?}", validator, missing_cars);

        match missing_cars {
            None => info!("{} no missing cars", self.storage.id),
            Some(cars) => {
                if let Err(e) = self.send_sync_reply(validator, cars) {
                    error!("{:?}", e)
                }
            }
        }
    }

    async fn on_data_vote(&mut self, validator: &ValidatorPublicKey, data_proposal: DataProposal) {
        debug!("Vote received from validator {}", validator);
        if self
            .storage
            .new_data_vote(validator, &data_proposal)
            .is_some()
        {
            debug!("{} DataVote from {}", self.storage.id, validator)
        } else {
            error!("{} unexpected DataVote from {}", self.storage.id, validator)
        }
    }

    async fn on_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
    ) -> Result<()> {
        if data_proposal.txs.is_empty() {
            warn!(
                "received empty data proposal from {}, ignoring...",
                validator
            );
            return Ok(());
        }
        match self.storage.new_data_proposal(validator, &data_proposal)? {
            ProposalVerdict::Vote => {
                // Normal case, we receive a proposal we already have the parent in store
                self.send_vote(validator, data_proposal)
            }
            ProposalVerdict::Wait(last_index) => {
                //We dont have the parent, so we craft a sync demand
                debug!("Emitting sync request with local state {} last_available_index {:?} and data_proposal {}", self.storage, last_index, data_proposal);

                self.send_sync_request(validator, data_proposal, last_index)
            }
        }
    }

    async fn try_data_proposal(&mut self, poa: Option<Vec<ValidatorPublicKey>>) {
        let pending_txs = self.storage.flush_pending_txs();
        if pending_txs.is_empty() {
            return;
        }
        let tip_id = self.storage.add_data_to_local_lane(pending_txs.clone());
        let data_proposal = DataProposal {
            txs: pending_txs,
            pos: tip_id,
            parent: if tip_id == 1 { None } else { Some(tip_id - 1) },
            parent_poa: poa,
        };

        if let Err(e) = self.broadcast_data_proposal(data_proposal) {
            error!("{:?}", e);
        }
    }

    async fn check_data_proposal(&mut self) {
        if self.storage.tip_already_used() {
            return;
        }
        if let Some(cut) = self.storage.try_a_new_cut(self.validators.len()) {
            let poa = self.storage.tip_poa();
            self.try_data_proposal(poa).await;
            let total_txs = cut.txs.len();
            if let Err(e) = self
                .bus
                .send(MempoolEvent::NewCut(cut))
                .context("Cannot send message over channel")
            {
                error!("{:?}", e);
            } else {
                self.metrics.add_batch();
                self.metrics.snapshot_batched_tx(total_txs);
            }
        } else if let Some((tip, txs)) = self.storage.tip_data() {
            // No PoA means we rebroadcast the data proposal for non present voters
            let only_for = HashSet::from_iter(
                self.validators
                    .iter()
                    .filter(|pubkey| !tip.poa.contains(pubkey))
                    .cloned(),
            );
            if let Err(e) = self.broadcast_data_proposal_only_for(
                only_for,
                DataProposal {
                    txs,
                    pos: tip.pos,
                    parent: tip.parent,
                    parent_poa: None, // TODO: fetch parent votes
                },
            ) {
                error!("{:?}", e);
            }
        } else {
            // Genesis create a mono tx chunk and broadcast it
            self.try_data_proposal(None).await;
        }
    }

    fn on_new_tx(&mut self, tx: Transaction) {
        if let Err(e) = fees_checker::check_fees(&tx) {
            error!("Invalid fees for transaction {}: {:?}", tx.hash(), e);
            return;
        }
        debug!("Got new tx {}", tx.hash());
        self.metrics.add_api_tx("blob".to_string());
        self.storage.accumulate_tx(tx.clone());
        self.metrics
            .snapshot_pending_tx(self.storage.pending_txs.len());
    }

    fn broadcast_tx(&mut self, tx: Transaction) -> Result<()> {
        self.metrics.add_broadcasted_tx("blob".to_string());
        self.broadcast_net_message(MempoolNetMessage::NewTx(tx))?;
        Ok(())
    }

    fn broadcast_data_proposal(&mut self, data_proposal: DataProposal) -> Result<()> {
        if self.validators.is_empty() {
            return Ok(());
        }
        self.metrics
            .add_broadcasted_data_proposal("blob".to_string());
        self.broadcast_net_message(MempoolNetMessage::DataProposal(data_proposal))?;
        Ok(())
    }

    fn broadcast_data_proposal_only_for(
        &mut self,
        only_for: HashSet<ValidatorPublicKey>,
        data_proposal: DataProposal,
    ) -> Result<()> {
        self.metrics
            .add_broadcasted_data_proposal_only_for("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::broadcast_only_for(
                only_for,
                self.sign_net_message(MempoolNetMessage::DataProposal(data_proposal))?,
            ))
            .log_error("broadcasting data_proposal_only_for");
        Ok(())
    }

    fn send_vote(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
    ) -> Result<()> {
        self.metrics.add_sent_data_vote("blob".to_string());
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::DataVote(data_proposal),
        )?;
        Ok(())
    }

    fn send_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
        last_index: Option<usize>,
    ) -> Result<()> {
        self.metrics.add_sent_sync_request("blob".to_string());
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::SyncRequest(data_proposal, last_index),
        )?;
        Ok(())
    }

    fn send_sync_reply(&mut self, validator: &ValidatorPublicKey, cars: Vec<Car>) -> Result<()> {
        // cleanup previously tracked sent sync request
        self.metrics.add_sent_sync_reply("blob".to_string());
        self.send_net_message(validator.clone(), MempoolNetMessage::SyncReply(cars))?;
        Ok(())
    }

    #[inline(always)]
    fn broadcast_net_message(&mut self, net_message: MempoolNetMessage) -> Result<()> {
        let signed_msg = self.sign_net_message(net_message)?;
        let enum_variant_name: &'static str = (&signed_msg.msg).into();
        self.bus
            .send(OutboundMessage::broadcast(signed_msg))
            .context(format!(
                "Broadcasting MempoolNetMessage::{} msg on the bus",
                enum_variant_name
            ))?;
        Ok(())
    }

    #[inline(always)]
    fn send_net_message(
        &mut self,
        to: ValidatorPublicKey,
        net_message: MempoolNetMessage,
    ) -> Result<()> {
        let signed_msg = self.sign_net_message(net_message)?;
        let enum_variant_name: &'static str = (&signed_msg.msg).into();
        _ = self
            .bus
            .send(OutboundMessage::send(to, signed_msg))
            .context(format!(
                "Sending MempoolNetMessage::{} msg on the bus",
                enum_variant_name
            ))?;
        Ok(())
    }

    fn sign_net_message(&self, msg: MempoolNetMessage) -> Result<SignedWithKey<MempoolNetMessage>> {
        self.crypto.sign(msg)
    }
}
