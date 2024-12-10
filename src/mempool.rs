//! Mempool logic & pending transaction management.

use crate::{
    bus::{command_response::Query, BusClientSender, BusMessage},
    consensus::ConsensusEvent,
    genesis::GenesisEvent,
    handle_messages,
    mempool::storage::{DataProposal, Storage},
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
use storage::{Cut, DataProposalHash, DataProposalVerdict, LaneEntry};
use strum_macros::IntoStaticStr;
use tracing::{debug, info, warn};

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
    DataVote(DataProposalHash),
    PoAUpdate(DataProposalHash, Vec<SignedByValidator<MempoolNetMessage>>),
    SyncRequest(Option<DataProposalHash>, DataProposalHash),
    SyncReply(Vec<LaneEntry>),
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
                Ok(self.handle_querynewcut(validators))
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
    fn handle_querynewcut(&mut self, validators: &mut QueryNewCut) -> Cut {
        // TODO: Do we want to receive f as well and be responsible to select DataProposal that have enough signatures ?
        self.metrics.add_new_cut(validators);
        self.storage.new_cut(&validators.0)
    }

    fn handle_api_message(&mut self, command: RestApiMessage) -> Result<()> {
        match command {
            RestApiMessage::NewTx(tx) => self
                .on_new_tx(tx)
                .context("Received invalid transaction. Won't process it."),
        }
    }

    fn handle_data_proposal_management(&mut self) -> Result<()> {
        // Create new DataProposal with pending txs
        self.storage.new_data_proposal();

        // Check if latest DataProposal has enough signatures
        if let Some(lane_entry) = &self.storage.get_lane_latest_entry(&self.storage.id) {
            // If no signature, broadcast it to everyone
            if lane_entry.signatures.is_empty() {
                debug!(
                    "🚗 Broadcast DataProposal {} ({} validators, {} txs)",
                    lane_entry.data_proposal.hash(),
                    self.validators.len(),
                    lane_entry.data_proposal.txs.len()
                );
                if self.validators.is_empty() {
                    return Ok(());
                }
                self.metrics.add_data_proposal(&lane_entry.data_proposal);
                self.metrics.add_proposed_txs(&lane_entry.data_proposal);
                self.broadcast_net_message(MempoolNetMessage::DataProposal(
                    lane_entry.data_proposal.clone(),
                ))?;
            } else {
                // If None, rebroadcast it to every validator that has not yet signed it
                let validator_that_has_signed: Vec<&ValidatorPublicKey> = lane_entry
                    .signatures
                    .iter()
                    .map(|s| &s.signature.validator)
                    .collect();

                // No PoA means we rebroadcast the DataProposal for non present voters
                let only_for: HashSet<ValidatorPublicKey> = self
                    .validators
                    .iter()
                    .filter(|pubkey| !validator_that_has_signed.contains(pubkey))
                    .cloned()
                    .collect();

                if only_for.is_empty() {
                    return Ok(());
                }

                self.metrics.add_data_proposal(&lane_entry.data_proposal);
                self.metrics.add_proposed_txs(&lane_entry.data_proposal);
                debug!(
                    "🚗 Broadcast DataProposal {} (only for {} validators, {} txs)",
                    &lane_entry.data_proposal.hash(),
                    only_for.len(),
                    &lane_entry.data_proposal.txs.len()
                );
                self.broadcast_only_for_net_message(
                    only_for,
                    MempoolNetMessage::DataProposal(lane_entry.data_proposal.clone()),
                )?;
            }
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
                self.fetch_unknown_data_proposals(&cut)?;
                let txs = self.storage.collect_txs_from_lanes(cut);
                self.bus
                    .send(MempoolEvent::CommitBlock(txs, new_bonded_validators))
                    .context("Cannot send commitBlock message over channel")?;

                Ok(())
            }
        }
    }

    fn fetch_unknown_data_proposals(&mut self, cut: &Cut) -> Result<()> {
        // Detect all unknown data proposals
        for (validator, data_proposal_hash) in cut.iter() {
            if !self
                .storage
                .lane_has_data_proposal(validator, data_proposal_hash)
            {
                let latest_data_proposal = self
                    .storage
                    .get_lane_latest_data_proposal_hash(validator)
                    .cloned();

                // Send SyncRequest for all unknown data proposals
                self.send_sync_request(
                    validator,
                    latest_data_proposal.as_ref(),
                    data_proposal_hash.clone(),
                )?;
            }
        }
        Ok(())
    }

    fn handle_net_message(&mut self, msg: SignedByValidator<MempoolNetMessage>) -> Result<()> {
        let result = BlstCrypto::verify(&msg)?;

        if !result {
            self.metrics.signature_error("mempool");
            bail!("Invalid signature for message {:?}", msg);
        }

        let validator = &msg.signature.validator;
        if !self.validators.contains(validator) {
            bail!(
                "Received {} message from unknown validator {validator}. Only accepting {:?}",
                msg.msg,
                self.validators
            );
        }

        match msg.msg {
            MempoolNetMessage::DataProposal(mut data_proposal) => {
                self.on_data_proposal(validator, &mut data_proposal)?;
            }
            MempoolNetMessage::DataVote(ref data_proposal_hash) => {
                self.on_data_vote(&msg, data_proposal_hash)?;
            }
            MempoolNetMessage::PoAUpdate(data_proposal_hash, mut signatures) => {
                self.on_poa_update(validator, &data_proposal_hash, &mut signatures)?;
            }
            MempoolNetMessage::SyncRequest(from_data_proposal_hash, to_data_proposal_hash) => {
                self.on_sync_request(
                    validator,
                    from_data_proposal_hash.as_ref(),
                    &to_data_proposal_hash,
                )?;
            }
            MempoolNetMessage::SyncReply(lane_entries) => {
                self.on_sync_reply(validator, lane_entries)?;
            }
        }
        Ok(())
    }

    fn on_sync_reply(
        &mut self,
        validator: &ValidatorPublicKey,
        missing_lane_entries: Vec<LaneEntry>,
    ) -> Result<()> {
        info!("{} SyncReply from validator {validator}", self.storage.id);

        debug!(
            "{} adding {} missing lane entries to lane of {validator}",
            self.storage.id,
            missing_lane_entries.len()
        );

        self.storage
            .add_missing_lane_entries(validator, missing_lane_entries)?;

        let mut waiting_proposals = self.storage.get_waiting_data_proposals(validator)?;
        for wp in waiting_proposals.iter_mut() {
            self.on_data_proposal(validator, wp)
                .context("Consuming waiting data proposal")?;
        }
        Ok(())
    }

    fn on_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        from_data_proposal_hash: Option<&DataProposalHash>,
        to_data_proposal_hash: &DataProposalHash,
    ) -> Result<()> {
        info!(
            "{} SyncRequest received from validator {validator} for last_data_proposal_hash {:?}",
            self.storage.id, to_data_proposal_hash
        );

        let missing_lane_entries = self.storage.get_lane_entries_between_hashes(
            &self.storage.id,
            from_data_proposal_hash,
            to_data_proposal_hash,
        );

        match missing_lane_entries {
            None => info!("Can't send sync reply as there are no missing data proposals found between {:?} and {:?} for {}", to_data_proposal_hash, from_data_proposal_hash, self.storage.id),
            Some(lane_entries) if lane_entries.is_empty() => {}
            Some(lane_entries) => {
                debug!(
                    "Missing data proposals on {} are {:?}",
                    validator, lane_entries
                );
                self.send_sync_reply(validator, lane_entries)?;
            }
        }
        Ok(())
    }

    fn on_data_vote(
        &mut self,
        msg: &SignedByValidator<MempoolNetMessage>,
        data_proposal_hash: &DataProposalHash,
    ) -> Result<()> {
        let validator = &msg.signature.validator;
        debug!("{} Vote from {}", self.storage.id, validator);
        self.storage.on_data_vote(msg, data_proposal_hash)?;
        Ok(())
    }

    fn on_poa_update(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal_hash: &DataProposalHash,
        signatures: &mut Vec<SignedByValidator<MempoolNetMessage>>,
    ) -> Result<()> {
        debug!(
            "Received {} signatures for DataProposal {} of validator {}",
            signatures.len(),
            data_proposal_hash,
            validator
        );
        self.storage
            .on_poa_update(validator, data_proposal_hash, signatures)?;
        Ok(())
    }

    fn on_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &mut DataProposal,
    ) -> Result<()> {
        debug!(
            "Received DataProposal {:?} from {} ({} txs)",
            data_proposal.hash(),
            validator,
            data_proposal.txs.len()
        );
        let data_proposal_hash = data_proposal.hash();
        match self
            .storage
            .on_data_proposal(validator, data_proposal, &self.node_state)
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
                self.send_vote(validator, data_proposal_hash)?;
            }
            DataProposalVerdict::Wait(last_known_data_proposal_hash) => {
                //We dont have the parent, so we craft a sync demand
                debug!(
                    "Emitting sync request with local state {} last_known_data_proposal_hash {:?}",
                    self.storage, last_known_data_proposal_hash
                );

                self.send_sync_request(
                    validator,
                    last_known_data_proposal_hash.as_ref(),
                    data_proposal_hash,
                )?;
            }
            DataProposalVerdict::Refuse => {
                debug!("Refuse vote for DataProposal");
            }
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

    fn send_vote(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal_hash: DataProposalHash,
    ) -> Result<()> {
        self.metrics
            .add_proposal_vote(self.crypto.validator_pubkey(), validator);
        info!("🗳️ Sending vote for DataProposal {data_proposal_hash} to {validator}");
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::DataVote(data_proposal_hash),
        )?;
        Ok(())
    }

    fn send_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        from_data_proposal_hash: Option<&DataProposalHash>,
        to_data_proposal_hash: DataProposalHash,
    ) -> Result<()> {
        debug!(
            "🔍 Sending SyncRequest to {} for DataProposal from {:?} to {:?}",
            validator, from_data_proposal_hash, to_data_proposal_hash
        );
        self.metrics
            .add_sync_request(self.crypto.validator_pubkey(), validator);
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::SyncRequest(from_data_proposal_hash.cloned(), to_data_proposal_hash),
        )?;
        Ok(())
    }

    fn send_sync_reply(
        &mut self,
        validator: &ValidatorPublicKey,
        lane_entries: Vec<LaneEntry>,
    ) -> Result<()> {
        // cleanup previously tracked sent sync request
        self.metrics.add_sync_reply(
            self.crypto.validator_pubkey(),
            validator,
            lane_entries.len(),
        );
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::SyncReply(lane_entries),
        )?;
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
    use tokio::sync::broadcast::Receiver;

    pub struct MempoolTestCtx {
        pub name: String,
        pub out_receiver: Receiver<OutboundMessage>,
        pub mempool_event_receiver: Receiver<MempoolEvent>,
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
            let mempool_event_receiver = get_receiver::<MempoolEvent>(&shared_bus).await;

            let mempool = Self::build_mempool(&shared_bus, crypto).await;

            MempoolTestCtx {
                name: name.to_string(),
                out_receiver,
                mempool_event_receiver,
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

        pub fn gen_cut(&mut self, validators: &[ValidatorPublicKey]) -> Cut {
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
        pub fn assert_broadcast_only_for(
            &mut self,
            description: &str,
        ) -> SignedByValidator<MempoolNetMessage> {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .out_receiver
                .try_recv()
                .expect(format!("{description}: No message broadcasted").as_str());

            match rec {
                OutboundMessage::BroadcastMessageOnlyFor(_, net_msg) => {
                    if let NetMessage::MempoolMessage(msg) = net_msg {
                        msg
                    } else {
                        println!(
                            "{description}: Mempool OutboundMessage message is missing, found {}",
                            net_msg
                        );
                        self.assert_broadcast_only_for(description)
                    }
                }
                _ => {
                    println!(
                        "{description}: Broadcast OutboundMessage message is missing, found {:?}",
                        rec
                    );
                    self.assert_broadcast_only_for(description)
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
                        tracing::error!(
                            "{description}: Mempool OutboundMessage message is missing, found {}",
                            msg
                        );
                        self.assert_send(to, description)
                    }
                }
                _ => {
                    tracing::error!(
                        "{description}: Broadcast OutboundMessage message is missing, found {}",
                        to
                    );
                    self.assert_send(to, description)
                }
            }
        }

        #[track_caller]
        pub fn assert_commit_block(&mut self) -> (Vec<Transaction>, Vec<ValidatorPublicKey>) {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .mempool_event_receiver
                .try_recv()
                .expect("No CommitBlock event sent");

            match rec {
                MempoolEvent::CommitBlock(txs, new_bounded_validators) => {
                    (txs, new_bounded_validators)
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

        pub fn current_hash(&self) -> Option<DataProposalHash> {
            let lane = self
                .mempool
                .storage
                .lanes
                .get(&self.validator_pubkey())
                .expect("Could not get own lane");
            lane.current_hash().cloned()
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

        ctx.mempool.handle_data_proposal_management()?;

        let data_proposal = match ctx.assert_broadcast("DataProposal").msg {
            MempoolNetMessage::DataProposal(data_proposal) => data_proposal,
            _ => panic!("Expected DataProposal message"),
        };
        assert_eq!(data_proposal.txs, vec![register_tx.clone()]);

        // Assert that pending_tx has been flushed
        assert!(ctx.mempool.storage.pending_txs.is_empty());

        // Adding new validator
        let temp_crypto = BlstCrypto::new("validator1".into());
        ctx.mempool
            .validators
            .push(temp_crypto.validator_pubkey().clone());

        let signed_msg = temp_crypto.sign(MempoolNetMessage::DataVote(data_proposal.hash()))?;
        let _ = ctx.mempool.handle_net_message(SignedByValidator {
            msg: MempoolNetMessage::DataVote(data_proposal.hash()),
            signature: signed_msg.signature,
        });

        // Adding new validator
        ctx.mempool.validators.push(ValidatorPublicKey(
            "validator2".to_owned().as_bytes().to_vec(),
        ));

        ctx.mempool.handle_data_proposal_management()?;
        let data_proposal = match ctx.assert_broadcast_only_for("DataProposal").msg {
            MempoolNetMessage::DataProposal(data_proposal) => data_proposal,
            _ => panic!("Expected DataProposal message"),
        };
        assert_eq!(data_proposal.txs, vec![register_tx]);

        // Assert that pending_tx is still flushed
        assert!(ctx.mempool.storage.pending_txs.is_empty());

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_data_proposal() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
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
                assert_eq!(data_vote, data_proposal.hash())
            }
            _ => panic!("Expected DataProposal message"),
        };
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_data_proposal_vote_from_unexpected_validator() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
        };

        let temp_crypto = BlstCrypto::new("temp_crypto".into());
        let signed_msg = temp_crypto.sign(MempoolNetMessage::DataVote(data_proposal.hash()))?;
        assert!(ctx
            .mempool
            .handle_net_message(SignedByValidator {
                msg: MempoolNetMessage::DataVote(data_proposal.hash()),
                signature: signed_msg.signature,
            })
            .is_err());

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

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![make_register_contract_tx(ContractName("test1".to_owned()))],
        };
        let data_proposal_hash = data_proposal.hash();

        ctx.mempool.handle_data_proposal_management()?;

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into());
        ctx.mempool
            .validators
            .push(crypto2.validator_pubkey().clone());

        let signed_msg = crypto2.sign(MempoolNetMessage::DataVote(data_proposal_hash.clone()))?;

        ctx.mempool
            .handle_net_message(signed_msg)
            .expect("should handle net message");

        // Assert that we added the vote to the signatures
        assert_eq!(
            ctx.mempool
                .storage
                .lanes
                .get(ctx.mempool.crypto.validator_pubkey())
                .unwrap()
                .get_last_proposal()
                .unwrap()
                .signatures
                .len(),
            1
        );
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_vote_for_unknown_data_proposal() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool.handle_data_proposal_management()?;

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into());
        ctx.mempool
            .validators
            .push(crypto2.validator_pubkey().clone());

        let signed_msg = crypto2.sign(MempoolNetMessage::DataVote(DataProposalHash(
            "non_existent".to_owned(),
        )))?;

        assert!(ctx.mempool.handle_net_message(signed_msg).is_err());
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_sync_request() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool.handle_data_proposal_management()?;

        let data_proposal = match ctx.assert_broadcast("DataProposal").msg {
            MempoolNetMessage::DataProposal(data_proposal) => data_proposal,
            _ => panic!("Expected DataProposal message"),
        };

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into());
        ctx.mempool
            .validators
            .push(crypto2.validator_pubkey().clone());

        let signed_msg =
            crypto2.sign(MempoolNetMessage::SyncRequest(None, data_proposal.hash()))?;

        ctx.mempool
            .handle_net_message(signed_msg)
            .expect("should handle net message");

        // Assert that we send a SyncReply
        match ctx.assert_send(crypto2.validator_pubkey(), "SyncReply").msg {
            MempoolNetMessage::SyncReply(lane_entries) => {
                assert_eq!(lane_entries.len(), 1);
                assert_eq!(lane_entries[0].data_proposal, data_proposal);
            }
            _ => panic!("Expected SyncReply message"),
        };
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_sync_reply() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName("test1".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool.handle_data_proposal_management()?;

        let data_proposal = match ctx.assert_broadcast("DataProposal").msg {
            MempoolNetMessage::DataProposal(data_proposal) => data_proposal,
            _ => panic!("Expected DataProposal message"),
        };

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into());
        ctx.mempool
            .validators
            .push(crypto2.validator_pubkey().clone());

        let signed_msg = crypto2.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            signatures: vec![],
        }]))?;

        ctx.mempool
            .handle_net_message(signed_msg)
            .expect("should handle net message");

        // Assert that the lane entry was added
        let lane = ctx
            .mempool
            .storage
            .lanes
            .get(crypto2.validator_pubkey())
            .expect("Could not get lane");
        assert_eq!(lane.data_proposals.len(), 1);
        assert_eq!(
            lane.get_last_proposal().unwrap().data_proposal,
            data_proposal
        );

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_commit_cut() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending first transaction to mempool as RestApiMessage
        let register_tx1 = make_register_contract_tx(ContractName("test1".to_owned()));
        let register_tx2 = make_register_contract_tx(ContractName("test2".to_owned()));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx1.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool.handle_data_proposal_management()?;

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into());
        ctx.mempool
            .validators
            .push(crypto2.validator_pubkey().clone());

        // Sending second transaction to mempool as RestApiMessage
        let data_proposal2 = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![register_tx2.clone()],
        };

        // Simulate receiving the second data proposal from the new validator
        let signed_msg = crypto2.sign(MempoolNetMessage::DataProposal(data_proposal2.clone()))?;
        ctx.mempool
            .handle_net_message(signed_msg)
            .expect("should handle net message");

        let cut = ctx.mempool.handle_querynewcut(&mut QueryNewCut(vec![
            ctx.mempool.crypto.validator_pubkey().clone(),
            crypto2.validator_pubkey().clone(),
        ]));

        ctx.mempool
            .handle_consensus_event(ConsensusEvent::CommitCut {
                cut: cut.clone(),
                new_bonded_validators: vec![],
                validators: vec![
                    ctx.mempool.crypto.validator_pubkey().clone(),
                    crypto2.validator_pubkey().clone(),
                ],
            })
            .expect("should handle consensus event");

        // Assert that the transactions have been committed
        // On veut assert commit block pour récuperer les txs
        // Assert that we send a SyncReply
        let (txs, new_bounded_validators) = ctx.assert_commit_block();

        assert_eq!(txs.len(), 2);
        assert!(txs.contains(&register_tx1));
        assert!(txs.contains(&register_tx2));
        assert!(new_bounded_validators.is_empty());

        Ok(())
    }
}
