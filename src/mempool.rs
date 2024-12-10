//! Mempool logic & pending transaction management.

use crate::{
    bus::{command_response::Query, BusClientSender, BusMessage},
    consensus::ConsensusEvent,
    genesis::GenesisEvent,
    handle_messages,
    mempool::storage::{Car, DataProposal, Storage},
    model::{
        Hashable, SharedRunContext, Transaction, TransactionData, ValidatorPublicKey,
        VerifiedProofTransaction,
    },
    module_handle_messages,
    node_state::NodeState,
    p2p::network::{OutboundMessage, SignedByValidator},
    utils::{
        conf::SharedConf,
        crypto::{BlstCrypto, SharedBlstCrypto},
        logger::LogMe,
        modules::{module_bus_client, Module},
    },
};
use anyhow::{bail, Context, Result};
use api::RestApiMessage;
use bincode::{Decode, Encode};
use metrics::MempoolMetrics;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fmt::Display, path::PathBuf, str::FromStr, sync::Arc};
use storage::{CarHash, Cut, DataProposalVerdict};
use strum_macros::IntoStaticStr;
use tracing::{debug, error, info, warn};

pub mod api;
pub mod metrics;
pub mod storage;

#[derive(Debug, Clone)]
pub struct QueryNewCut(pub Vec<ValidatorPublicKey>);

module_bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(MempoolEvent),
    receiver(SignedByValidator<MempoolNetMessage>),
    receiver(RestApiMessage),
    receiver(ConsensusEvent),
    receiver(GenesisEvent),
    receiver(Query<QueryNewCut, Cut>),
}
}

pub struct Mempool {
    bus: MempoolBusClient,
    file: Option<PathBuf>,
    conf: SharedConf,
    crypto: SharedBlstCrypto,
    metrics: MempoolMetrics,
    storage: Storage,
    validators: Vec<ValidatorPublicKey>,
    node_state: NodeState,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum MempoolNetMessage {
    DataProposal(DataProposal),
    DataVote(CarHash),
    // FIXME: Add a new message to receive a Car's PoA
    SyncRequest(CarHash, Option<CarHash>),
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
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = MempoolBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        let metrics = MempoolMetrics::global(ctx.common.config.id.clone());

        let node_state = Self::load_from_disk_or_default::<NodeState>(
            ctx.common
                .config
                .data_directory
                .join("node_state.bin")
                .as_path(),
        );

        let api = api::api(&ctx.common).await;
        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/", api));
            }
        }
        let storage = Self::load_from_disk::<Storage>(
            ctx.common
                .config
                .data_directory
                .join("mempool_storage.bin")
                .as_path(),
        )
        .unwrap_or(Storage::new(ctx.node.crypto.validator_pubkey().clone()));
        Ok(Mempool {
            bus,
            file: Some(PathBuf::from_str("mempool_storage.bin").unwrap()),
            conf: ctx.common.config.clone(),
            metrics,
            crypto: Arc::clone(&ctx.node.crypto),
            storage,
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

        // Recompute optimistic node_state

        module_handle_messages! {
            on_bus self.bus,
            listen<SignedByValidator<MempoolNetMessage>> cmd => {
                let _ = self.handle_net_message(cmd)
                    .log_error("Handling MempoolNetMessage in Mempool");
            }
            listen<RestApiMessage> cmd => {
                let _ = self.handle_api_message(cmd)
                    .log_error("Handling RestApiMessage in Mempool");
            }
            listen<ConsensusEvent> cmd => {
                let _ = self.handle_consensus_event(cmd)
                    .log_error("Handling ConsensusEvent in Mempool");
            }
            listen<GenesisEvent> cmd => {
                if let GenesisEvent::GenesisBlock { genesis_txs, .. } = cmd {
                    for tx in genesis_txs {
                        if let TransactionData::RegisterContract(tx) = tx.transaction_data {
                            self.node_state.handle_register_contract_tx(&tx)?;
                        }
                    }
                }
            }
            command_response<QueryNewCut, Cut> validators => {
                self.handle_querynewcut(validators)
                    .log_error("Handling QueryNewCut")
            }
            _ = interval.tick() => {
                let _ = self.handle_data_proposal_management()
                    .log_error("Creating Data Proposal on tick");
            }
        }

        if let Some(file) = &self.file {
            if let Err(e) = Self::save_on_disk(
                self.conf.data_directory.as_path(),
                file.as_path(),
                &self.storage,
            ) {
                warn!("Failed to save mempool storage on disk: {}", e);
            }
        }

        Ok(())
    }

    /// Creates a cut with local material on QueryNewCut message reception (from consensus)
    fn handle_querynewcut(&mut self, validators: &mut QueryNewCut) -> Result<Cut> {
        // TODO: metrics?
        self.metrics.add_new_cut(validators);
        // FIXME: use voting power
        // self.validators.len() == 1 is for SingleNodeBlockGeneration
        //   it makes sure the DataProposal is committed to the Lane without waiting for a DataVote
        // self.storage.lane.poa.len() > f is for waiting for the appropriate number of votes
        //   it makes sure we received enough DataVote before commiting the DataProposal to the Lane.
        let f = validators.0.len() / 3;
        if validators.0.len() == 1 || self.storage.lane.poa.len() > f {
            self.storage.commit_data_proposal();
        }
        Ok(self.storage.new_cut(&validators.0))
    }

    fn handle_api_message(&mut self, command: RestApiMessage) -> Result<()> {
        match command {
            RestApiMessage::NewTx(tx) => self
                .on_new_tx(tx)
                .context("Received invalid transaction. Won't process it."),
        }
    }

    fn handle_data_proposal_management(&mut self) -> Result<()> {
        // FIXME: Split this flow in three steps:
        // 1: create new DataProposal with pending txs and broadcast it as a DataProposal.
        // 2: Save DataProposal. It is not yet a Car (since PoA is not reached)
        // 3: Go through all pending DataProposal (of own lane) that have not reach PoA, and send them to make them Cars
        self.broadcast_data_proposal_if_any()?;
        if let Some(data_proposal) = &self.storage.data_proposal {
            // No PoA means we rebroadcast the DataProposal for non present voters
            let only_for = HashSet::from_iter(
                self.validators
                    .iter()
                    .filter(|pubkey| !self.storage.lane.poa.contains(pubkey))
                    .cloned(),
            );
            // FIXME: with current implem, we send DataProposal twice.
            self.metrics.add_data_proposal(data_proposal);
            self.broadcast_only_for_net_message(
                only_for,
                MempoolNetMessage::DataProposal(data_proposal.clone()),
            )?;
        }
        Ok(())
    }

    fn handle_consensus_event(&mut self, event: ConsensusEvent) -> Result<()> {
        match event {
            ConsensusEvent::CommitCut {
                cut,
                new_bonded_validators,
                validators,
            } => {
                debug!("✂️ Received CommitCut ({:?} cut)", cut);
                self.validators = validators;
                let txs = self.storage.collect_lanes(cut);
                self.bus
                    .send(MempoolEvent::CommitBlock(txs, new_bonded_validators))
                    .context("Cannot send commitBlock message over channel")?;

                Ok(())
            }
        }
    }

    fn handle_net_message(&mut self, msg: SignedByValidator<MempoolNetMessage>) -> Result<()> {
        let result = BlstCrypto::verify(&msg)?;

        if !result {
            self.metrics.signature_error("mempool");
            bail!("Invalid signature for message {:?}", msg);
        }

        let validator = &msg.signature.validator;
        match msg.msg {
            MempoolNetMessage::DataProposal(data_proposal) => {
                self.on_data_proposal(validator, data_proposal)?;
            }
            MempoolNetMessage::DataVote(car_hash) => {
                // FIXME: We should extract the signature for that Vote in order to create a PoA
                self.on_data_vote(validator, car_hash)?;
            }
            MempoolNetMessage::SyncRequest(data_proposal_tip_hash, last_known_car_hash) => {
                self.on_sync_request(validator, &data_proposal_tip_hash, last_known_car_hash)?;
            }
            MempoolNetMessage::SyncReply(cars) => {
                self
                    // TODO: we don't know who sent the message
                    .on_sync_reply(validator, cars)?;
            }
        }
        Ok(())
    }

    fn on_sync_reply(
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
            .other_lane_add_missing_cars(validator, missing_cars)?;

        let waiting_proposals = self.storage.get_waiting_data_proposals(validator)?;
        for wp in waiting_proposals {
            self.on_data_proposal(validator, wp)
                .context("Consuming waiting data proposal")?;
        }
        Ok(())
    }

    fn on_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal_tip_hash: &CarHash,
        last_known_car_hash: Option<CarHash>,
    ) -> Result<()> {
        info!(
            "{} SyncRequest received from validator {validator} for last_car_hash {:?}",
            self.storage.id, last_known_car_hash
        );

        let missing_cars = self
            .storage
            .get_missing_cars(last_known_car_hash, data_proposal_tip_hash);

        match missing_cars {
            None => info!("{} no missing cars", self.storage.id),
            Some(cars) if cars.is_empty() => {}
            Some(cars) => {
                debug!("Missing cars on {} are {:?}", validator, cars);
                self.send_sync_reply(validator, cars)?;
            }
        }
        Ok(())
    }

    fn on_data_vote(&mut self, validator: &ValidatorPublicKey, car_hash: CarHash) -> Result<()> {
        self.storage.on_data_vote(validator, &car_hash)?;
        debug!("{} Vote from {}", self.storage.id, validator);
        if self.storage.lane.poa.len() > self.validators.len() / 3 {
            self.storage.commit_data_proposal();
        }
        Ok(())
    }

    fn on_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
    ) -> Result<()> {
        debug!(
            "Received DataProposal (car: {:?}) from {}",
            data_proposal.car.hash(),
            validator
        );
        match self
            .storage
            .on_data_proposal(validator, &data_proposal, &self.node_state)
        {
            DataProposalVerdict::Empty => {
                warn!(
                    "received empty DataProposal from {}, ignoring...",
                    validator
                );
            }
            DataProposalVerdict::Vote => {
                // Normal case, we receive a proposal we already have the parent in store
                debug!("Send vote for DataProposal");
                self.send_vote(validator, data_proposal.car.hash())?;
            }
            DataProposalVerdict::DidVote => {
                error!(
                    "we already have voted for {}'s DataProposal {}",
                    validator,
                    data_proposal.car.hash()
                );
            }
            DataProposalVerdict::Wait(last_known_car_hash) => {
                //We dont have the parent, so we craft a sync demand
                debug!(
                    "Emitting sync request with local state {} last_known_car_hash {:?}",
                    self.storage, last_known_car_hash
                );

                self.send_sync_request(validator, data_proposal, last_known_car_hash)?;
            }
            DataProposalVerdict::Refuse => {
                debug!("Refuse vote for DataProposal");
            }
        }
        Ok(())
    }

    fn broadcast_data_proposal_if_any(&mut self) -> Result<()> {
        if let Some(data_proposal) = self.storage.new_data_proposal() {
            debug!(
                "🚗 Broadcast DataProposal {} ({} validators, {} txs)",
                data_proposal.car.hash(),
                self.validators.len(),
                data_proposal.car.txs.len()
            );
            if self.validators.is_empty() {
                return Ok(());
            }
            self.metrics.add_data_proposal(&data_proposal);
            self.metrics.add_proposed_txs(&data_proposal);
            self.broadcast_net_message(MempoolNetMessage::DataProposal(data_proposal))?;
        }
        Ok(())
    }

    fn on_new_tx(&mut self, mut tx: Transaction) -> Result<()> {
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
                let hyle_output = self.node_state.verify_proof(&proof_transaction)?;
                tx.transaction_data = TransactionData::VerifiedProof(VerifiedProofTransaction {
                    proof_transaction,
                    hyle_output,
                });
            }
            TransactionData::VerifiedProof(_) => {
                bail!("Already verified ProofTransaction are not allowed to be received in the mempool");
            }
        }

        let tx_type: &'static str = (&tx.transaction_data).into();

        self.metrics.add_api_tx(tx_type);
        self.storage.on_new_tx(tx);
        self.metrics
            .snapshot_pending_tx(self.storage.pending_txs.len());

        Ok(())
    }

    fn send_vote(&mut self, validator: &ValidatorPublicKey, car_hash: CarHash) -> Result<()> {
        self.metrics
            .add_proposal_vote(self.crypto.validator_pubkey(), validator);
        self.send_net_message(validator.clone(), MempoolNetMessage::DataVote(car_hash))?;
        Ok(())
    }

    fn send_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
        last_known_car_hash: Option<CarHash>,
    ) -> Result<()> {
        self.metrics
            .add_sync_request(self.crypto.validator_pubkey(), validator);
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::SyncRequest(data_proposal.car.hash(), last_known_car_hash),
        )?;
        Ok(())
    }

    fn send_sync_reply(&mut self, validator: &ValidatorPublicKey, cars: Vec<Car>) -> Result<()> {
        // cleanup previously tracked sent sync request
        self.metrics
            .add_sync_reply(self.crypto.validator_pubkey(), validator, cars.len());
        self.send_net_message(validator.clone(), MempoolNetMessage::SyncReply(cars))?;
        Ok(())
    }

    #[inline(always)]
    fn broadcast_net_message(&mut self, net_message: MempoolNetMessage) -> Result<()> {
        let signed_msg = self.sign_net_message(net_message)?;
        let enum_variant_name: &'static str = (&signed_msg.msg).into();
        let error_msg =
            format!("Broadcasting MempoolNetMessage::{enum_variant_name} msg on the bus");
        self.bus
            .send(OutboundMessage::broadcast(signed_msg))
            .context(error_msg)?;
        Ok(())
    }

    #[inline(always)]
    fn broadcast_only_for_net_message(
        &mut self,
        only_for: HashSet<ValidatorPublicKey>,
        net_message: MempoolNetMessage,
    ) -> Result<()> {
        let signed_msg = self.sign_net_message(net_message)?;
        let enum_variant_name: &'static str = (&signed_msg.msg).into();
        let error_msg = format!(
            "Broadcasting MempoolNetMessage::{} msg only for: {:?} on the bus",
            enum_variant_name, only_for
        );
        self.bus
            .send(OutboundMessage::broadcast_only_for(only_for, signed_msg))
            .context(error_msg)?;
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
        let error_msg = format!("Sending MempoolNetMessage::{enum_variant_name} msg on the bus");
        _ = self
            .bus
            .send(OutboundMessage::send(to, signed_msg))
            .context(error_msg)?;
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
pub mod test {
    use super::*;
    use crate::bus::dont_use_this::get_receiver;
    use crate::bus::metrics::BusMetrics;
    use crate::bus::SharedMessageBus;
    use crate::model::{ContractName, RegisterContractTransaction, Transaction};
    use crate::p2p::network::NetMessage;
    use anyhow::Result;
    use hyle_contract_sdk::StateDigest;
    use std::collections::BTreeSet;
    use storage::Poa;
    use tokio::sync::broadcast::Receiver;

    pub struct MempoolTestCtx {
        pub name: String,
        pub out_receiver: Receiver<OutboundMessage>,
        pub mempool: Mempool,
    }

    impl MempoolTestCtx {
        pub async fn build_mempool(shared_bus: &SharedMessageBus, crypto: BlstCrypto) -> Mempool {
            let storage = Storage::new(crypto.validator_pubkey().clone());
            let validators = vec![crypto.validator_pubkey().clone()];
            let bus = MempoolBusClient::new_from_bus(shared_bus.new_handle()).await;

            // Initialize Mempool
            Mempool {
                bus,
                file: None,
                conf: SharedConf::default(),
                crypto: Arc::new(crypto),
                metrics: MempoolMetrics::global("id".to_string()),
                storage,
                validators,
                node_state: NodeState::default(),
            }
        }

        pub async fn new(name: &str) -> Self {
            let crypto = BlstCrypto::new(name.into());
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));

            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;

            let mempool = Self::build_mempool(&shared_bus, crypto).await;

            MempoolTestCtx {
                name: name.to_string(),
                out_receiver,
                mempool,
            }
        }

        pub fn setup_node(&mut self, cryptos: &[BlstCrypto]) {
            for other_crypto in cryptos.iter() {
                self.mempool
                    .validators
                    .push(other_crypto.validator_pubkey().clone());
            }
        }

        pub fn validator_pubkey(&self) -> ValidatorPublicKey {
            self.mempool.crypto.validator_pubkey().clone()
        }

        pub fn gen_cut(&mut self, validators: &[ValidatorPublicKey]) -> Result<Cut> {
            self.mempool
                .handle_querynewcut(&mut QueryNewCut(validators.to_vec()))
        }

        pub fn make_data_proposal_with_pending_txs(&mut self) -> Result<()> {
            self.mempool.handle_data_proposal_management()
        }

        pub fn submit_tx(&mut self, tx: &Transaction) {
            self.mempool
                .handle_api_message(RestApiMessage::NewTx(tx.clone()))
                .expect("fail to handle new transaction");
        }

        #[track_caller]
        pub fn assert_broadcast(
            &mut self,
            description: &str,
        ) -> SignedByValidator<MempoolNetMessage> {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .out_receiver
                .try_recv()
                .expect(format!("{description}: No message broadcasted").as_str());

            match rec {
                OutboundMessage::BroadcastMessage(net_msg) => {
                    if let NetMessage::MempoolMessage(msg) = net_msg {
                        msg
                    } else {
                        println!(
                            "{description}: Mempool OutboundMessage message is missing, found {}",
                            net_msg
                        );
                        self.assert_broadcast(description)
                    }
                }
                _ => {
                    println!(
                        "{description}: Broadcast OutboundMessage message is missing, found {:?}",
                        rec
                    );
                    self.assert_broadcast(description)
                }
            }
        }

        #[track_caller]
        pub fn assert_send(
            &mut self,
            to: &ValidatorPublicKey,
            description: &str,
        ) -> SignedByValidator<MempoolNetMessage> {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .out_receiver
                .try_recv()
                .expect(format!("{description}: No message broadcasted").as_str());

            match rec {
                OutboundMessage::SendMessage { validator_id, msg } => {
                    if &validator_id != to {
                        panic!(
                            "{description}: Send message was sent to {validator_id} instead of {}",
                            to
                        );
                    }
                    if let NetMessage::MempoolMessage(msg) = msg {
                        info!("received message: {:?}", msg);
                        msg
                    } else {
                        error!(
                            "{description}: Mempool OutboundMessage message is missing, found {}",
                            msg
                        );
                        self.assert_send(to, description)
                    }
                }
                _ => {
                    error!(
                        "{description}: Broadcast OutboundMessage message is missing, found {}",
                        to
                    );
                    self.assert_send(to, description)
                }
            }
        }

        #[track_caller]
        pub fn handle_msg(&mut self, msg: &SignedByValidator<MempoolNetMessage>, _err: &str) {
            debug!("📥 {} Handling message: {:?}", self.name, msg);
            self.mempool
                .handle_net_message(msg.clone())
                .expect("should handle net msg");
        }

        pub fn current_hash(&self) -> Option<CarHash> {
            self.mempool.storage.lane.current_hash()
        }
    }

    pub fn make_register_contract_tx(name: ContractName) -> Transaction {
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
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool
            .broadcast_data_proposal_if_any()
            .expect("should broadcast of any");

        let data_proposal = match ctx.assert_broadcast("DataProposal").msg {
            MempoolNetMessage::DataProposal(data_proposal) => data_proposal,
            _ => panic!("Expected DataProposal message"),
        };
        assert_eq!(data_proposal.car.txs, vec![register_tx]);

        // Assert that pending_tx has been flushed
        assert!(ctx.mempool.storage.pending_txs.is_empty());

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_data_proposal() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
            },
            parent_poa: None,
        };

        let signed_msg = ctx
            .mempool
            .crypto
            .sign(MempoolNetMessage::DataProposal(data_proposal.clone()))?;
        ctx.mempool
            .handle_net_message(SignedByValidator {
                msg: MempoolNetMessage::DataProposal(data_proposal.clone()),
                signature: signed_msg.signature,
            })
            .expect("should handle net message");

        // Assert that we vote for that specific DataProposal
        match ctx
            .assert_send(&ctx.mempool.crypto.validator_pubkey().clone(), "DataVote")
            .msg
        {
            MempoolNetMessage::DataVote(data_vote) => {
                assert_eq!(data_vote, data_proposal.car.hash())
            }
            _ => panic!("Expected DataProposal message"),
        };
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_unexpect_data_proposal_vote() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        assert_eq!(
            ctx.mempool.storage.lane.poa,
            Poa(BTreeSet::from([ctx
                .mempool
                .crypto
                .validator_pubkey()
                .clone()]))
        );

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: Some(CarHash("42".to_string())), // This value is incorrect
                txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
            },
            parent_poa: None,
        };

        let temp_crypto = BlstCrypto::new("temp_crypto".into());
        let signed_msg = temp_crypto.sign(MempoolNetMessage::DataVote(data_proposal.car.hash()))?;
        ctx.mempool
            .handle_net_message(SignedByValidator {
                msg: MempoolNetMessage::DataVote(data_proposal.car.hash()),
                signature: signed_msg.signature,
            })
            .expect("should handle net message");

        // Assert that we did not add the vote to the PoA
        assert_eq!(
            ctx.mempool.storage.lane.poa,
            Poa(BTreeSet::from([ctx
                .mempool
                .crypto
                .validator_pubkey()
                .clone(),]))
        );
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_data_proposal_vote() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        assert_eq!(
            ctx.mempool.storage.lane.poa,
            Poa(BTreeSet::from([ctx
                .mempool
                .crypto
                .validator_pubkey()
                .clone()]))
        );

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
            },
            parent_poa: None,
        };
        ctx.mempool
            .broadcast_data_proposal_if_any()
            .expect("should broadcast if any");

        let temp_crypto = BlstCrypto::new("temp_crypto".into());
        let signed_msg = temp_crypto.sign(MempoolNetMessage::DataVote(data_proposal.car.hash()))?;
        ctx.mempool
            .handle_net_message(SignedByValidator {
                msg: MempoolNetMessage::DataVote(data_proposal.car.hash()),
                signature: signed_msg.signature,
            })
            .expect("should handle net message");

        // Assert that we added the vote to the PoA
        assert_eq!(
            ctx.mempool.storage.lane.poa,
            Poa(BTreeSet::from([
                ctx.mempool.crypto.validator_pubkey().clone(),
                temp_crypto.validator_pubkey().clone()
            ]))
        );
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_data_proposal_management() -> Result<()> {
        // TODO: on veut rajouter ces Car à la main avec trop peu de PoA.

        let mut ctx = MempoolTestCtx::new("mempool").await;

        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));
        ctx.mempool.storage.pending_txs.push(register_tx.clone());

        ctx.mempool
            .handle_data_proposal_management()
            .expect("should create data proposal");

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![register_tx],
            },
            parent_poa: None,
        };

        // Assert that we vote for that specific DataProposal
        match ctx.assert_broadcast("DataVote").msg {
            MempoolNetMessage::DataProposal(received_data_proposal) => {
                assert_eq!(received_data_proposal, data_proposal)
            }
            _ => panic!("Expected DataProposal message"),
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
        let mut ctx = MempoolTestCtx::new("mempool").await;
        let car_hash = CarHash("42".to_string());
        let cut: Cut = vec![(
            ctx.mempool.crypto.validator_pubkey().clone(),
            car_hash.clone(),
        )];

        ctx.mempool
            .handle_consensus_event(ConsensusEvent::CommitCut {
                cut: cut.clone(),
                new_bonded_validators: vec![],
                validators: vec![ctx.mempool.crypto.validator_pubkey().clone()],
            })
            .expect("should handle consensus event");

        let car = ctx.mempool.storage.lane.current().expect("No tip info");

        assert_eq!(car.hash(), car_hash);
        Ok(())
    }
}
