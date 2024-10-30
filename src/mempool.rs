//! Mempool logic & pending transaction management.

use crate::{
    bus::{bus_client, BusMessage, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    mempool::storage::{Car, CarProposal, InMemoryStorage},
    model::{Hashable, SharedRunContext, Transaction, TransactionData, ValidatorPublicKey},
    node_state::NodeState,
    p2p::network::{OutboundMessage, SignedByValidator},
    rest::endpoints::RestApiMessage,
    utils::{
        crypto::{BlstCrypto, SharedBlstCrypto},
        logger::LogMe,
        modules::Module,
    },
};
use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use metrics::MempoolMetrics;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fmt::Display, sync::Arc};
use storage::{CarId, ProposalVerdict};
use strum_macros::IntoStaticStr;
use tracing::{debug, error, info, warn};

mod metrics;
mod storage;
pub use storage::Cut;

bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(MempoolEvent),
    receiver(SignedByValidator<MempoolNetMessage>),
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
    node_state: NodeState,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum MempoolNetMessage {
    NewCut(Cut),
    CarProposal(CarProposal),
    CarProposalVote(CarProposal),
    SyncRequest(CarProposal, Option<CarId>),
    SyncReply(Vec<Car>),
}

impl Display for MempoolNetMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let enum_variant: &'static str = self.into();
        write!(f, "{}", enum_variant)
    }
}

impl BusMessage for MempoolNetMessage {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum MempoolEvent {
    NewCut(Cut),
    CommitBlock(Vec<Transaction>, Vec<ValidatorPublicKey>),
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

        let node_state = Self::load_from_disk_or_default::<NodeState>(
            ctx.common
                .config
                .data_directory
                .join("mempool_node_state.bin")
                .as_path(),
        );

        Ok(Mempool {
            bus,
            metrics,
            crypto: Arc::clone(&ctx.node.crypto),
            storage: InMemoryStorage::new(ctx.node.crypto.validator_pubkey().clone()),
            validators: vec![],
            node_state,
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
            listen<SignedByValidator<MempoolNetMessage>> cmd => {
                self.handle_net_message(cmd).await
            }
            listen<RestApiMessage> cmd => {
                match self.handle_api_message(cmd).await {
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling RestApi message: {:#}", e),
                }
            }
            listen<ConsensusEvent> cmd => {
                self.handle_consensus_event(cmd).await
            }
            _ = interval.tick() => {
                self.time_to_cut().await
            }
        }
    }

    async fn handle_consensus_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitCut {
                cut,
                new_bonded_validators,
                validators,
            } => {
                debug!(
                    "✂️ Received CommitCut ({:?} cut, {} pending txs)",
                    cut,
                    self.storage.pending_txs.len()
                );
                self.validators = validators;
                let txs = self.storage.update_lanes_after_commit(cut);
                if let Err(e) = self
                    .bus
                    .send(MempoolEvent::CommitBlock(txs, new_bonded_validators))
                    .context("Cannot send message over channel")
                {
                    error!("{:?}", e);
                };
            }
            ConsensusEvent::GenesisBlock { .. } => {}
        }
    }

    async fn handle_net_message(&mut self, msg: SignedByValidator<MempoolNetMessage>) {
        match BlstCrypto::verify(&msg) {
            Ok(true) => {
                let validator = &msg.signature.validator;
                match msg.msg {
                    MempoolNetMessage::NewCut(cut) => {
                        self.send_new_cut(cut);
                    }
                    MempoolNetMessage::CarProposal(car_proposal) => {
                        if let Err(e) = self.on_car_proposal(validator, car_proposal).await {
                            error!("{:?}", e);
                        }
                    }
                    MempoolNetMessage::CarProposalVote(car_proposal) => {
                        self.on_proposal_vote(validator, car_proposal).await;
                    }
                    MempoolNetMessage::SyncRequest(car_proposal, last_car_id) => {
                        self.on_sync_request(validator, car_proposal, last_car_id)
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

    async fn handle_api_message(&mut self, command: RestApiMessage) -> Result<(), Error> {
        match command {
            RestApiMessage::NewTx(tx) => {
                if let Err(e) = self.on_new_tx(tx) {
                    bail!("Received invalid transaction: {:?}. Won't process it.", e);
                }
            }
        };
        Ok(())
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
        last_car_id: Option<CarId>,
    ) {
        info!(
            "{} SyncRequest received from validator {validator} for last_car_id {:?}",
            self.storage.id, last_car_id
        );

        let missing_cars = self.storage.get_missing_cars(last_car_id, &car_proposal);

        match missing_cars {
            None => info!("{} no missing cars", self.storage.id),
            Some(cars) if cars.is_empty() => {}
            Some(cars) => {
                debug!("Missing cars on {} are {:?}", validator, cars);
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
            debug!("{} Vote from {}", self.storage.id, validator)
        } else {
            error!("{} unexpected Vote from {}", self.storage.id, validator)
        }
    }

    async fn on_car_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: CarProposal,
    ) -> Result<()> {
        match self
            .storage
            .new_car_proposal(validator, &car_proposal, &self.node_state)
        {
            ProposalVerdict::Empty => {
                warn!(
                    "received empty Car proposal from {}, ignoring...",
                    validator
                );
            }
            ProposalVerdict::Vote => {
                // Normal case, we receive a proposal we already have the parent in store
                debug!("Send vote for Car proposal");
                self.send_vote(validator, car_proposal)?;
            }
            ProposalVerdict::DidVote => {
                error!("we already have voted for {}'s Car proposal", validator);
            }
            ProposalVerdict::Wait(last_car_id) => {
                //We dont have the parent, so we craft a sync demand
                debug!(
                    "Emitting sync request with local state {} last_available_index {:?}",
                    self.storage, last_car_id
                );

                self.send_sync_request(validator, car_proposal, last_car_id)?;
            }
            ProposalVerdict::Refuse => {}
        }
        Ok(())
    }

    fn try_car_proposal(&mut self, poa: Option<Vec<ValidatorPublicKey>>) {
        if let Some(car_proposal) = self.storage.try_car_proposal(poa) {
            debug!(
                "🚗 Broadcast Car proposal ({} validators, {} txs)",
                self.validators.len(),
                car_proposal.txs.len()
            );
            if let Err(e) = self.broadcast_car_proposal(car_proposal) {
                error!("{:?}", e);
            }
        }
    }

    fn send_new_cut(&mut self, cut: Cut) {
        if let Err(e) = self
            .bus
            .send(MempoolEvent::NewCut(cut))
            .context("Cannot send NewCut over channel")
        {
            error!("{:?}", e);
        } else {
            self.metrics.add_batch();
        }
    }

    async fn time_to_cut(&mut self) {
        if let Some(cut) = self.storage.try_new_cut(&self.validators) {
            let poa = self.storage.tip_poa();
            self.try_car_proposal(poa);
            if let Err(e) = self
                .broadcast_net_message(MempoolNetMessage::NewCut(cut.clone()))
                .context("Cannot broadcast NewCut message")
            {
                error!("{:?}", e);
            }
            self.send_new_cut(cut);
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

    fn on_new_tx(&mut self, mut tx: Transaction) -> Result<(), Error> {
        debug!("Got new tx {}", tx.hash());
        // TODO: Verify fees ?
        // TODO: Verify identity ?

        match tx.transaction_data {
            TransactionData::RegisterContract(ref register_contract_transaction) => {
                self.node_state
                    .handle_register_contract_tx(register_contract_transaction)?;
            }
            TransactionData::Stake(ref _staker) => {}
            TransactionData::Blob(ref _blob_transaction) => {}
            TransactionData::Proof(proof_transaction) => {
                // Verify and extract proof
                let verified_proof_tx = proof_transaction.verify(&self.node_state)?;
                tx.transaction_data = TransactionData::VerifiedProof(verified_proof_tx);
            }
            TransactionData::VerifiedProof(_) => {
                bail!("Already verified ProofTransaction are not allowed to be received in the mempool");
            }
        }

        self.metrics.add_api_tx("blob".to_string());
        self.storage.add_new_tx(tx);
        if self.storage.genesis() {
            // Genesis create and broadcast a new Car proposal
            self.try_car_proposal(None);
        }
        self.metrics
            .snapshot_pending_tx(self.storage.pending_txs.len());

        Ok(())
    }

    fn broadcast_car_proposal(&mut self, car_proposal: CarProposal) -> Result<()> {
        if self.validators.is_empty() {
            return Ok(());
        }
        self.metrics
            .add_broadcasted_car_proposal("blob".to_string());
        self.broadcast_net_message(MempoolNetMessage::CarProposal(car_proposal))?;
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
                self.sign_net_message(MempoolNetMessage::CarProposal(car_proposal))?,
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
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::CarProposalVote(car_proposal),
        )?;
        Ok(())
    }

    fn send_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: CarProposal,
        last_car_id: Option<CarId>,
    ) -> Result<()> {
        self.metrics.add_sent_sync_request("blob".to_string());
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::SyncRequest(car_proposal, last_car_id),
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

    fn sign_net_message(
        &self,
        msg: MempoolNetMessage,
    ) -> Result<SignedByValidator<MempoolNetMessage>> {
        self.crypto.sign(msg)
    }
}
