//! Mempool logic & pending transaction management.

use crate::{
    bus::{bus_client, command_response::Query, BusMessage, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    mempool::storage::{CarProposal, InMemoryStorage},
    model::{
        Hashable, SharedRunContext, Transaction, TransactionData, ValidatorPublicKey,
        VerifiedProofTransaction,
    },
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
use storage::{Car, CarId, Cut, ProposalVerdict};
use strum_macros::IntoStaticStr;
use tracing::{debug, error, info, warn};

mod metrics;
pub mod storage;
#[derive(Debug, Clone)]
pub struct QueryNewCut(pub Vec<ValidatorPublicKey>);

bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(MempoolEvent),
    receiver(SignedByValidator<MempoolNetMessage>),
    receiver(RestApiMessage),
    receiver(ConsensusEvent),
    receiver(Query<QueryNewCut, Cut>),
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
    CarProposal(CarProposal),
    CarProposalVote(CarProposal),
    // FIXME: Add a new message to receive a Car's PoA
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
                if let Err(e) = self.handle_api_message(cmd).await {
                    warn!("Error while handling RestApi message: {:#}", e);
                }
            }
            listen<ConsensusEvent> cmd => {
                self.handle_consensus_event(cmd).await
            }
            command_response<QueryNewCut, Cut> validators => {
                // TODO: metrics?
                self.metrics.add_batch();
                Ok(self.storage.make_new_cut(&validators.0))
            }
            _ = interval.tick() => {
                // This tick is responsible for CarProposal management
                self.handle_car_proposal_management();
            }
        }
    }

    fn handle_car_proposal_management(&mut self) {
        // FIXME: Split this flow in three steps:
        // 1: create new CarProposal with pending txs and broadcast it as a CarProposal.
        // 2: Save CarProposal. It is not yet a Car (since PoA is not reached)
        // 3: Go through all pending CarProposal (of own lane) that have not reach PoA, and send them to make them Cars
        let poa = self.storage.tip_poa();
        self.try_car_proposal(poa);
        if let Some((tip, txs)) = self.storage.tip_info() {
            // No PoA means we rebroadcast the Car proposal for non present voters
            let only_for = HashSet::from_iter(
                self.validators
                    .iter()
                    .filter(|pubkey| !tip.poa.contains(pubkey))
                    .cloned(),
            );
            // FIXME: with current implem, we send CarProposal twice.
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
                    MempoolNetMessage::CarProposal(car_proposal) => {
                        debug!(
                            "Received CarProposal {} from validator {}",
                            car_proposal.id, validator
                        );
                        if let Err(e) = self.on_car_proposal(validator, car_proposal).await {
                            error!("{:?}", e);
                        }
                    }
                    MempoolNetMessage::CarProposalVote(car_proposal) => {
                        // FIXME: We should extract the signature for that Vote in order to create a PoA
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
                error!(
                    "we already have voted for {}'s Car proposal {}",
                    validator, car_proposal.id
                );
            }
            ProposalVerdict::Wait(last_car_id) => {
                //We dont have the parent, so we craft a sync demand
                debug!(
                    "Emitting sync request with local state {} last_available_index {:?}",
                    self.storage, last_car_id
                );

                self.send_sync_request(validator, car_proposal, last_car_id)?;
            }
            ProposalVerdict::Refuse => {
                debug!("Refuse vote for Car proposal");
            }
        }
        Ok(())
    }

    fn try_car_proposal(&mut self, poa: Option<Vec<ValidatorPublicKey>>) {
        if let Some(car_proposal) = self.storage.try_car_proposal(poa) {
            debug!(
                "🚗 Broadcast Car proposal {} ({} validators, {} txs)",
                car_proposal.id,
                self.validators.len(),
                car_proposal.txs.len()
            );
            if let Err(e) = self.broadcast_car_proposal(car_proposal) {
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
                let hyle_outputs = self.node_state.verify_proof(&proof_transaction)?;
                tx.transaction_data = TransactionData::VerifiedProof(VerifiedProofTransaction {
                    proof_transaction,
                    hyle_outputs,
                });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::SharedMessageBus;
    use crate::mempool::MempoolBusClient;
    use crate::model::{ContractName, RegisterContractTransaction, Transaction};
    use crate::p2p::network::NetMessage;
    use anyhow::Result;
    use hyle_contract_sdk::StateDigest;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use storage::Poa;
    use tokio::sync::broadcast::Receiver;

    pub struct TestContext {
        out_receiver: Receiver<OutboundMessage>,
        mempool: Mempool,
    }

    impl TestContext {
        pub async fn new(name: &str) -> Self {
            let crypto = BlstCrypto::new(name.into());
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let storage = InMemoryStorage::new(crypto.validator_pubkey().clone());
            let validators = vec![crypto.validator_pubkey().clone()];

            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let bus = MempoolBusClient::new_from_bus(shared_bus.new_handle()).await;

            // Initialize Mempool
            let mempool = Mempool {
                bus,
                crypto: Arc::new(crypto),
                metrics: MempoolMetrics::global("id".to_string()),
                storage,
                validators,
                node_state: NodeState::default(),
            };

            TestContext {
                out_receiver,
                mempool,
            }
        }

        #[track_caller]
        fn assert_broadcast(&mut self, err: &str) -> MempoolNetMessage {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .out_receiver
                .try_recv()
                .expect(format!("{err}: No message broadcasted").as_str());

            match rec {
                OutboundMessage::BroadcastMessage(net_msg) => {
                    if let NetMessage::MempoolMessage(msg) = net_msg {
                        msg.msg
                    } else {
                        panic!("{err}: Mempool OutboundMessage message is missing");
                    }
                }
                OutboundMessage::SendMessage {
                    validator_id: _,
                    msg,
                } => {
                    if let NetMessage::MempoolMessage(msg) = msg {
                        tracing::error!("recieved message: {:?}", msg);
                        msg.msg
                    } else {
                        panic!("{err}: Mempool OutboundMessage message is missing");
                    }
                }
                _ => panic!("{err}: Broadcast OutboundMessage message is missing"),
            }
        }
    }

    fn make_register_contract_tx(name: ContractName) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::RegisterContract(RegisterContractTransaction {
                owner: "test".to_string(),
                verifier: "test".to_string(),
                program_id: vec![],
                state_digest: StateDigest(vec![0, 1, 2, 3]),
                contract_name: name,
            }),
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_new_tx() -> Result<()> {
        let mut ctx = TestContext::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .await
            .expect("fail to handle new transaction");

        let car_proposal = match ctx.assert_broadcast("Car Proposal") {
            MempoolNetMessage::CarProposal(car_proposal) => car_proposal,
            _ => panic!("Expected CarProposal message"),
        };
        assert_eq!(car_proposal.txs, vec![register_tx]);

        // Assert that pending_tx has been flushed
        assert!(ctx.mempool.storage.pending_txs.is_empty());

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_car_proposal() -> Result<()> {
        let mut ctx = TestContext::new("mempool").await;

        let car_proposal = CarProposal {
            txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
            id: CarId(1),
            parent: None,
            parent_poa: None,
        };

        let signed_msg = ctx
            .mempool
            .crypto
            .sign(MempoolNetMessage::CarProposal(car_proposal.clone()))?;
        ctx.mempool
            .handle_net_message(SignedByValidator {
                msg: MempoolNetMessage::CarProposal(car_proposal.clone()),
                signature: signed_msg.signature,
            })
            .await;

        // Assert that we vote for that specific CarProposal
        match ctx.assert_broadcast("Car Proposal Vote") {
            MempoolNetMessage::CarProposalVote(car_proposal_vote) => {
                assert_eq!(car_proposal_vote, car_proposal)
            }
            _ => panic!("Expected CarProposal message"),
        };
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_unexpect_car_proposal_vote() -> Result<()> {
        let mut ctx = TestContext::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .await
            .expect("fail to handle new transaction");

        assert_eq!(
            ctx.mempool.storage.lane.current().unwrap().poa,
            Poa(BTreeSet::from([ctx
                .mempool
                .crypto
                .validator_pubkey()
                .clone()]))
        );

        let car_proposal = CarProposal {
            txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
            id: CarId(10), // This value is incorrect
            parent: None,
            parent_poa: None,
        };

        let temp_crypto = BlstCrypto::new("temp_crypto".into());
        let signed_msg =
            temp_crypto.sign(MempoolNetMessage::CarProposalVote(car_proposal.clone()))?;
        ctx.mempool
            .handle_net_message(SignedByValidator {
                msg: MempoolNetMessage::CarProposalVote(car_proposal.clone()),
                signature: signed_msg.signature,
            })
            .await;

        // Assert that we did not add the vote to the PoA
        assert_eq!(
            ctx.mempool.storage.lane.current().unwrap().poa,
            Poa(BTreeSet::from([ctx
                .mempool
                .crypto
                .validator_pubkey()
                .clone(),]))
        );
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_car_proposal_vote() -> Result<()> {
        let mut ctx = TestContext::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .await
            .expect("fail to handle new transaction");

        assert_eq!(
            ctx.mempool.storage.lane.current().unwrap().poa,
            Poa(BTreeSet::from([ctx
                .mempool
                .crypto
                .validator_pubkey()
                .clone()]))
        );

        let car_proposal = CarProposal {
            txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
            id: CarId(1),
            parent: None,
            parent_poa: None,
        };

        let temp_crypto = BlstCrypto::new("temp_crypto".into());
        let signed_msg =
            temp_crypto.sign(MempoolNetMessage::CarProposalVote(car_proposal.clone()))?;
        ctx.mempool
            .handle_net_message(SignedByValidator {
                msg: MempoolNetMessage::CarProposalVote(car_proposal.clone()),
                signature: signed_msg.signature,
            })
            .await;

        // Assert that we added the vote to the PoA
        assert_eq!(
            ctx.mempool.storage.lane.current().unwrap().poa,
            Poa(BTreeSet::from([
                ctx.mempool.crypto.validator_pubkey().clone(),
                temp_crypto.validator_pubkey().clone()
            ]))
        );
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_car_proposal_management() -> Result<()> {
        // TODO: on veut rajouter ces Car à la main avec trop peu de PoA.

        let mut ctx = TestContext::new("mempool").await;

        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));
        ctx.mempool.storage.pending_txs.push(register_tx.clone());

        ctx.mempool.handle_car_proposal_management();

        let car_proposal = CarProposal {
            txs: vec![register_tx],
            id: CarId(1),
            parent: None,
            parent_poa: None,
        };

        // Assert that we vote for that specific CarProposal
        match ctx.assert_broadcast("Car Proposal Vote") {
            MempoolNetMessage::CarProposal(received_car_proposal) => {
                assert_eq!(received_car_proposal, car_proposal)
            }
            _ => panic!("Expected CarProposal message"),
        };

        Ok(())
    }

    #[ignore = "TODO"]
    #[test_log::test(tokio::test)]
    async fn test_receiving_sync_request() -> Result<()> {
        Ok(())
    }

    #[ignore = "TODO"]
    #[test_log::test(tokio::test)]
    async fn test_receiving_sync_reply() -> Result<()> {
        Ok(())
    }

    #[ignore = "TODO"]
    #[test_log::test(tokio::test)]
    async fn test_receiving_commit_cut() -> Result<()> {
        let mut ctx = TestContext::new("mempool").await;
        let car_id = CarId(1);
        let cut: Cut = vec![(ctx.mempool.crypto.validator_pubkey().clone(), car_id)];

        ctx.mempool
            .handle_consensus_event(ConsensusEvent::CommitCut {
                cut: cut.clone(),
                new_bonded_validators: vec![],
                validators: vec![ctx.mempool.crypto.validator_pubkey().clone()],
            })
            .await;

        let (tip_info, _) = ctx.mempool.storage.tip_info().expect("No tip info");

        assert_eq!(tip_info.pos, car_id);
        Ok(())
    }
}
