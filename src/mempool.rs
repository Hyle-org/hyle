//! Mempool logic & pending transaction management.

use crate::{
    bus::{bus_client, BusMessage, SharedMessageBus},
    consensus::{
        staking::{Stake, Staker},
        ConsensusEvent,
    },
    handle_messages,
    mempool::storage::{Car, CarProposal, InMemoryStorage},
    model::{Hashable, SharedRunContext, Transaction, TransactionData, ValidatorPublicKey},
    p2p::network::{OutboundMessage, PeerEvent, SignedWithKey},
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

mod metrics;
mod storage;
pub use storage::{Cut, CutWithTxs};

bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(MempoolEvent),
    receiver(SignedWithKey<MempoolNetMessage>),
    receiver(RestApiMessage),
    receiver(ConsensusEvent),
    receiver(PeerEvent),
}
}

pub struct Mempool {
    bus: MempoolBusClient,
    crypto: SharedBlstCrypto,
    metrics: MempoolMetrics,
    storage: InMemoryStorage,
    validators: Vec<ValidatorPublicKey>,
    genesis: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
    DataProposal(CarProposal),
    DataVote(CarProposal),
    SyncRequest(CarProposal, Option<usize>),
    SyncReply(Vec<Car>),
}
impl BusMessage for MempoolNetMessage {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum MempoolEvent {
    NewCut(CutWithTxs),
}
impl BusMessage for MempoolEvent {}

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
            genesis: true,
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

        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(100));

        handle_messages! {
            on_bus self.bus,
            listen<PeerEvent> cmd => {
                self.handle_peer_event(cmd)
            }
            listen<SignedWithKey<MempoolNetMessage>> cmd => {
                self.handle_net_message(cmd).await
            }
            listen<RestApiMessage> cmd => {
                self.handle_api_message(cmd).await
            }
            listen<ConsensusEvent> cmd => {
                self.handle_consensus_event(cmd).await
            }
            _ = interval.tick() => {
                self.time_for_a_cut().await
            }
        }
    }

    async fn handle_consensus_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitCut {
                validators, cut, ..
            } => {
                self.genesis = false;
                self.validators = validators;
                self.storage.update_lanes_after_commit(cut);
            }
        }
    }

    fn add_stake_tx_on_genesis_for(&mut self, pubkey: ValidatorPublicKey) {
        let tx = Transaction::wrap(TransactionData::Stake(Staker {
            pubkey: pubkey.clone(),
            stake: Stake { amount: 100 },
        }));
        self.on_new_tx(tx);
        self.validators.push(pubkey);
    }

    fn handle_peer_event(&mut self, event: PeerEvent) {
        match event {
            PeerEvent::NewPeer { pubkey } => {
                if self.genesis {
                    self.add_stake_tx_on_genesis_for(pubkey);
                }
            }
        }
    }

    async fn handle_net_message(&mut self, msg: SignedWithKey<MempoolNetMessage>) {
        match BlstCrypto::verify(&msg) {
            Ok(true) => {
                let validator = msg.validators.first().unwrap();
                match msg.msg {
                    MempoolNetMessage::NewTx(tx) => self.on_new_tx(tx),
                    MempoolNetMessage::DataProposal(car_proposal) => {
                        if let Err(e) = self.on_car_proposal(validator, car_proposal).await {
                            error!("{:?}", e);
                        }
                    }
                    MempoolNetMessage::DataVote(car_proposal) => {
                        self.on_proposal_vote(validator, car_proposal).await;
                    }
                    MempoolNetMessage::SyncRequest(car_proposal, last_index) => {
                        self.on_sync_request(validator, car_proposal, last_index)
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

        self.storage
            .other_lane_add_missing_cars(validator, missing_cars);

        let waiting_proposals = self.storage.get_waiting_proposals(validator);
        for wp in waiting_proposals {
            if let Err(e) = self.on_car_proposal(validator, wp).await {
                error!("{:?}", e);
            }
        }
        Ok(())
    }

    async fn on_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: CarProposal,
        last_index: Option<usize>,
    ) {
        info!(
            "{} SyncRequest received from validator {validator} for last_index {:?}",
            self.storage.id, last_index
        );

        let missing_cars = self.storage.get_missing_cars(last_index, &car_proposal);
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

    async fn on_proposal_vote(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: CarProposal,
    ) {
        debug!("Vote received from validator {}", validator);
        if self
            .storage
            .new_vote_for_proposal(validator, &car_proposal)
            .is_some()
        {
            debug!("{} DataVote from {}", self.storage.id, validator)
        } else {
            error!("{} unexpected DataVote from {}", self.storage.id, validator)
        }
    }

    async fn on_car_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: CarProposal,
    ) -> Result<()> {
        match self.storage.new_car_proposal(validator, &car_proposal) {
            ProposalVerdict::Empty => {
                warn!(
                    "received empty Car proposal from {}, ignoring...",
                    validator
                );
            }
            ProposalVerdict::Vote => {
                // Normal case, we receive a proposal we already have the parent in store
                self.send_vote(validator, car_proposal)?;
            }
            ProposalVerdict::DidVote => {
                error!("we already have voted for {}'s Car proposal", validator);
            }
            ProposalVerdict::Wait(last_index) => {
                //We dont have the parent, so we craft a sync demand
                debug!("Emitting sync request with local state {} last_available_index {:?} and car_proposal {}", self.storage, last_index, car_proposal);

                self.send_sync_request(validator, car_proposal, last_index)?;
            }
        }
        Ok(())
    }

    fn try_car_proposal(&mut self, poa: Option<Vec<ValidatorPublicKey>>) {
        if let Some(car_proposal) = self.storage.try_car_proposal(poa) {
            if let Err(e) = self.broadcast_car_proposal(car_proposal) {
                error!("{:?}", e);
            }
        }
    }

    async fn time_for_a_cut(&mut self) {
        if self.storage.tip_already_used() {
            return;
        }
        if let Some(cut) = self.storage.try_new_cut(self.validators.len()) {
            let poa = self.storage.tip_poa();
            self.try_car_proposal(poa);
            if let Err(e) = self
                .bus
                .send(MempoolEvent::NewCut(cut))
                .context("Cannot send message over channel")
            {
                error!("{:?}", e);
            } else {
                self.metrics.add_batch();
            }
        } else if let Some((tip, txs)) = self.storage.tip_info() {
            // No PoA means we rebroadcast the Car proposal for non present voters
            let only_for = HashSet::from_iter(
                self.validators
                    .iter()
                    .filter(|pubkey| !tip.poa.contains(pubkey))
                    .cloned(),
            );
            if let Err(e) = self.broadcast_car_proposal_only_for(
                only_for,
                CarProposal {
                    txs,
                    id: tip.pos,
                    parent: tip.parent,
                    parent_poa: None, // TODO: fetch parent votes
                },
            ) {
                error!("{:?}", e);
            }
        }
    }

    fn on_new_tx(&mut self, tx: Transaction) {
        debug!("Got new tx {}", tx.hash());
        self.metrics.add_api_tx("blob".to_string());
        self.storage.add_new_tx(tx.clone());
        if self.storage.genesis() {
            // Genesis create and broadcast a new Car proposal
            self.try_car_proposal(None);
        }
        self.metrics
            .snapshot_pending_tx(self.storage.pending_txs.len());
    }

    fn broadcast_tx(&mut self, tx: Transaction) -> Result<()> {
        self.metrics.add_broadcasted_tx("blob".to_string());
        self.broadcast_net_message(MempoolNetMessage::NewTx(tx))?;
        Ok(())
    }

    fn broadcast_car_proposal(&mut self, car_proposal: CarProposal) -> Result<()> {
        if self.validators.is_empty() {
            return Ok(());
        }
        self.metrics
            .add_broadcasted_car_proposal("blob".to_string());
        self.broadcast_net_message(MempoolNetMessage::DataProposal(car_proposal))?;
        Ok(())
    }

    fn broadcast_car_proposal_only_for(
        &mut self,
        only_for: HashSet<ValidatorPublicKey>,
        car_proposal: CarProposal,
    ) -> Result<()> {
        self.metrics
            .add_broadcasted_car_proposal_only_for("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::broadcast_only_for(
                only_for,
                self.sign_net_message(MempoolNetMessage::DataProposal(car_proposal))?,
            ))
            .log_error("broadcasting car_proposal_only_for");
        Ok(())
    }

    fn send_vote(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: CarProposal,
    ) -> Result<()> {
        self.metrics.add_sent_proposal_vote("blob".to_string());
        self.send_net_message(validator.clone(), MempoolNetMessage::DataVote(car_proposal))?;
        Ok(())
    }

    fn send_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: CarProposal,
        last_index: Option<usize>,
    ) -> Result<()> {
        self.metrics.add_sent_sync_request("blob".to_string());
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::SyncRequest(car_proposal, last_index),
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
