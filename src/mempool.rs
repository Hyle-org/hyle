//! Mempool logic & pending transaction management.

use crate::{
    bus::{command_response::Query, BusClientSender, BusMessage},
    consensus::{CommittedConsensusProposal, ConsensusEvent},
    genesis::GenesisEvent,
    mempool::storage::Storage,
    model::{
        BlobProofOutput, Hashable, ProofDataHash, SharedRunContext, Transaction, TransactionData,
        ValidatorPublicKey, VerifiedProofTransaction,
    },
    module_handle_messages,
    node_state::module::NodeStateEvent,
    p2p::network::OutboundMessage,
    tcp_server::TcpServerMessage,
    utils::{
        conf::SharedConf,
        crypto::{BlstCrypto, SharedBlstCrypto, SignedByValidator},
        logger::LogMe,
        modules::{module_bus_client, Module},
        static_type_map::Pick,
    },
};

use anyhow::{bail, Context, Result};
use api::RestApiMessage;
use bincode::{Decode, Encode};
use hyle_contract_sdk::{ContractName, ProgramId, Verifier};
use metrics::MempoolMetrics;
use serde::{Deserialize, Serialize};
use staking::state::Staking;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
    path::PathBuf,
    sync::Arc,
};
use storage::{DataProposalVerdict, LaneEntry};
use strum_macros::IntoStaticStr;
use tracing::{debug, error, info, warn};

use verifiers::{verify_proof, verify_recursive_proof};

pub use crate::model::mempool::*;

pub mod api;
pub mod metrics;
pub mod storage;
pub mod verifiers;

#[derive(Debug, Clone)]
pub struct QueryNewCut(pub Staking);

#[derive(Debug, Default, Clone, Encode, Decode)]
pub struct KnownContracts(pub HashMap<ContractName, (Verifier, ProgramId)>);

impl KnownContracts {
    #[inline(always)]
    fn register_contract(
        &mut self,
        contract_name: &ContractName,
        verifier: &Verifier,
        program_id: &ProgramId,
    ) -> Result<()> {
        if self.0.contains_key(contract_name) {
            bail!("Contract {contract_name} already exists")
        }
        self.0.insert(
            contract_name.clone(),
            (verifier.clone(), program_id.clone()),
        );
        Ok(())
    }
}

module_bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(MempoolEvent),
    sender(InternalMempoolEvent),
    receiver(InternalMempoolEvent),
    receiver(SignedByValidator<MempoolNetMessage>),
    receiver(RestApiMessage),
    receiver(TcpServerMessage),
    receiver(MempoolCommand),
    receiver(ConsensusEvent),
    receiver(GenesisEvent),
    receiver(NodeStateEvent),
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
    da_latest_pending_cuts: VecDeque<(Option<Cut>, Cut)>,
    validators: Vec<ValidatorPublicKey>,
    known_contracts: Arc<std::sync::RwLock<KnownContracts>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum MempoolNetMessage {
    DataProposal(DataProposal),
    DataVote(DataProposalHash),
    PoDAUpdate(DataProposalHash, Vec<SignedByValidator<MempoolNetMessage>>),
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
    DataProposals(Cut, Vec<(ValidatorPublicKey, Vec<DataProposal>)>),
}
impl BusMessage for MempoolEvent {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum MempoolCommand {
    FetchDataProposals { from: Option<Cut>, to: Cut },
}
impl BusMessage for MempoolCommand {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum InternalMempoolEvent {
    OnProcessedNewTx(Transaction),
    OnProcessedDataProposal((ValidatorPublicKey, DataProposalVerdict, DataProposal)),
}
impl BusMessage for InternalMempoolEvent {}

impl Module for Mempool {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = MempoolBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        let metrics = MempoolMetrics::global(ctx.common.config.id.clone());

        let known_contracts = Arc::new(std::sync::RwLock::new(Self::load_from_disk_or_default::<
            KnownContracts,
        >(
            ctx.common
                .config
                .data_directory
                .join("mempool_known_contracts.bin")
                .as_path(),
        )));

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
            file: Some(ctx.common.config.data_directory.join("mempool_storage.bin")),
            conf: ctx.common.config.clone(),
            metrics,
            crypto: Arc::clone(&ctx.node.crypto),
            storage,
            da_latest_pending_cuts: VecDeque::new(),
            validators: vec![],
            known_contracts,
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

        let tick_time = std::cmp::min(self.conf.consensus.slot_duration / 2, 500);
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(tick_time));

        // TODO: Recompute optimistic node_state

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
            listen<TcpServerMessage> cmd => {
                let _ = self.handle_tcp_server_message(cmd)
                    .log_error("Handling TcpServerNetMessage in Mempool");
            }
            listen<MempoolCommand> cmd => {
                let _ = self.handle_command(cmd).log_error("Handling Mempool Command");
            }
            listen<InternalMempoolEvent> event => {
                let _ = self.handle_internal_event(event)
                    .log_error("Handling InternalMempoolEvent in Mempool");
            }
            listen<ConsensusEvent> cmd => {
                let _ = self.handle_consensus_event(cmd)
                    .log_error("Handling ConsensusEvent in Mempool");
            }
            listen<GenesisEvent> cmd => {
                if let GenesisEvent::GenesisBlock(signed_block) = cmd {
                    for tx in signed_block.txs() {
                        if let TransactionData::RegisterContract(tx) = tx.transaction_data {
                            #[allow(clippy::expect_used, reason="not held across await")]
                            self.known_contracts.write().expect("logic issue").register_contract(&tx.contract_name, &tx.verifier, &tx.program_id)?;
                        }
                    }
                }
            }
            listen<NodeStateEvent> cmd => {
                let NodeStateEvent::NewBlock(block) = cmd;
                for tx in block.txs {
                    let TransactionData::RegisterContract(register_contract_transaction) = tx.transaction_data else {
                        continue;
                    };
                    #[allow(clippy::expect_used, reason="not held across await")]
                    let _ = self.known_contracts.write().expect("logic issue").register_contract(
                        &register_contract_transaction.contract_name,
                        &register_contract_transaction.verifier,
                        &register_contract_transaction.program_id,
                    );
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
            if let Err(e) = Self::save_on_disk(file.as_path(), &self.storage) {
                warn!("Failed to save mempool storage on disk: {}", e);
            }
        }

        Ok(())
    }

    fn handle_command(&mut self, cmd: MempoolCommand) -> Result<()> {
        match cmd {
            MempoolCommand::FetchDataProposals { from, to } => {
                self.fetch_unknown_data_proposals(&to)
                    .context("Fetching Data Proposals")?;
                // Handle blocks in wrong order (here we suppose they arrive in the right order)
                self.da_latest_pending_cuts.push_back((from, to));

                self.try_fetch_queued_data_proposals()
                    .context("Handling FetchDataProposals")
            }
        }
    }

    /// Creates a cut with local material on QueryNewCut message reception (from consensus)
    fn handle_querynewcut(&mut self, staking: &mut QueryNewCut) -> Cut {
        self.metrics.add_new_cut(staking);
        self.storage.new_cut(&staking.0)
    }

    fn handle_api_message(&mut self, command: RestApiMessage) -> Result<()> {
        match command {
            RestApiMessage::NewTx(tx) => self
                .on_new_tx(tx)
                .context("Received invalid transaction. Won't process it."),
        }
    }

    fn handle_tcp_server_message(&mut self, command: TcpServerMessage) -> Result<()> {
        match command {
            TcpServerMessage::NewTx(tx) => self
                .on_new_tx(tx)
                .context("Received invalid transaction. Won't process it."),
        }
    }

    fn handle_internal_event(&mut self, event: InternalMempoolEvent) -> Result<()> {
        match event {
            InternalMempoolEvent::OnProcessedNewTx(tx) => {
                self.on_new_tx(tx).context("Processing new tx")
            }
            InternalMempoolEvent::OnProcessedDataProposal((validator, verdict, data_proposal)) => {
                self.on_processed_data_proposal(validator, verdict, data_proposal)
                    .context("Processing data proposal")
            }
        }
    }

    fn handle_data_proposal_management(&mut self) -> Result<()> {
        debug!("🌝 Handling DataProposal management");
        // Create new DataProposal with pending txs
        self.storage.new_data_proposal(&self.crypto); // TODO: copy crypto in storage

        // Check if latest DataProposal has enough signatures
        if let Some(lane_entry) = &self.storage.get_lane_latest_entry(&self.storage.id) {
            // If there's only 1 signature (=own signature), broadcast it to everyone
            if lane_entry.signatures.len() == 1 && self.validators.len() > 1 {
                debug!(
                    "🚗 Broadcast DataProposal {} ({} validators, {} txs)",
                    lane_entry.data_proposal.id,
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
                    &lane_entry.data_proposal.id,
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

    /// Emits a DataProposals event containing data proposals between the two cuts provided.
    /// If data is not available locally, fails and do nothing
    fn handle_fetch_data_proposals(&mut self, from: Option<&Cut>, to: &Cut) -> Result<()> {
        let mut result: Vec<(ValidatorPublicKey, Vec<DataProposal>)> = vec![];
        // Try to return the asked data proposals
        for (validator, to_hash, _) in to {
            // FIXME: use from : &Cut instead of Option
            let from_hash = from
                .and_then(|f| f.iter().find(|el| &el.0 == validator))
                .map(|el| &el.1);

            let entries = self
                .storage
                .get_lane_entries_between_hashes(
                    validator, // get start hash for validator
                    from_hash, to_hash,
                )
                .context(format!(
                    "Lane entries from {:?} to {:?} not available locally",
                    from, to
                ))?;

            result.push((
                validator.clone(),
                entries
                    .unwrap_or_default()
                    .into_iter()
                    .map(|e| e.data_proposal)
                    .collect(),
            ))
        }

        self.bus
            .send(MempoolEvent::DataProposals(to.clone(), result))
            .context("Sending DataProposals")?;
        Ok(())
    }

    fn try_fetch_queued_data_proposals(&mut self) -> Result<()> {
        while let Some((from, to)) = self.da_latest_pending_cuts.pop_front() {
            if let Err(e) = self.handle_fetch_data_proposals(from.as_ref(), &to) {
                error!("{:?}", e);
                // if failure, we push back the cut to provide data for it later
                self.da_latest_pending_cuts.push_front((from, to));
                break;
            }
        }

        Ok(())
    }

    fn handle_consensus_event(&mut self, event: ConsensusEvent) -> Result<()> {
        match event {
            ConsensusEvent::CommitConsensusProposal(CommittedConsensusProposal {
                validators,
                consensus_proposal,
                certificate: _,
            }) => {
                debug!(
                    "✂️ Received CommittedConsensusProposal (slot {}, {:?} cut)",
                    consensus_proposal.slot, consensus_proposal.cut
                );

                self.validators = validators;

                // Fetch in advance data proposals
                self.fetch_unknown_data_proposals(&consensus_proposal.cut)?;

                // Update all lanes with the new cut
                self.storage
                    .update_lanes_with_commited_cut(&consensus_proposal.cut);

                Ok(())
            }
        }
    }

    fn fetch_unknown_data_proposals(&mut self, cut: &Cut) -> Result<()> {
        // Detect all unknown data proposals
        for (validator, data_proposal_hash, _) in cut.iter() {
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
                )
                .context("Fetching unknown data")?;
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
        // TODO: adapt can_rejoin test to emit a stake tx before turning on the joining node
        // if !self.validators.contains(validator) {
        //     bail!(
        //         "Received {} message from unknown validator {validator}. Only accepting {:?}",
        //         msg.msg,
        //         self.validators
        //     );
        // }

        match msg.msg {
            MempoolNetMessage::DataProposal(data_proposal) => {
                self.on_data_proposal(validator, data_proposal)?;
            }
            MempoolNetMessage::DataVote(ref data_proposal_hash) => {
                self.on_data_vote(&msg, data_proposal_hash)?;
            }
            MempoolNetMessage::PoDAUpdate(data_proposal_hash, mut signatures) => {
                self.on_poda_update(validator, &data_proposal_hash, &mut signatures)?;
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
        mut missing_lane_entries: Vec<LaneEntry>,
    ) -> Result<()> {
        info!("{} SyncReply from validator {validator}", self.storage.id);

        // Discard any lane entry that wasn't signed by the validator.
        missing_lane_entries.retain(|lane_entry| {
            let expected_message = MempoolNetMessage::DataVote(lane_entry.data_proposal.hash());
            let keep = lane_entry
                .signatures
                .iter()
                .any(|s| &s.signature.validator == validator && s.msg == expected_message);
            if !keep {
                warn!(
                    "Discarding lane entry {}, missing signature from {}",
                    lane_entry.data_proposal.hash(),
                    validator
                );
            };
            keep
        });

        // If we end up with an empty list, return an error (for testing/logic)
        if missing_lane_entries.is_empty() {
            bail!("Empty lane entries after filtering out missing signatures");
        }

        debug!(
            "{} adding {} missing lane entries to lane of {validator}",
            self.storage.id,
            missing_lane_entries.len()
        );

        self.storage
            .add_missing_lane_entries(validator, missing_lane_entries)?;

        let mut waiting_proposals = self.storage.get_waiting_data_proposals(validator)?;
        for wp in waiting_proposals.iter_mut() {
            self.on_data_proposal(validator, std::mem::take(wp))
                .context("Consuming waiting data proposal")?;
        }

        self.try_fetch_queued_data_proposals()
            .context("Handling FetchDataProposals on sync reply")?;

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
            Err(e) => info!("Can't send sync reply as there are no missing data proposals found between {:?} and {:?} for {}: {}", to_data_proposal_hash, from_data_proposal_hash, self.storage.id, e),
            Ok(None) => {}
            Ok(Some(lane_entries)) => {
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

    fn on_poda_update(
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
            .on_poda_update(validator, data_proposal_hash, signatures)?;
        Ok(())
    }

    fn on_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        mut data_proposal: DataProposal,
    ) -> Result<()> {
        debug!(
            "Received DataProposal {:?} from {} ({} txs)",
            data_proposal.hash(),
            validator,
            data_proposal.txs.len()
        );
        let data_proposal_hash = data_proposal.hash();
        match self.storage.on_data_proposal(validator, &data_proposal) {
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
            DataProposalVerdict::Process => {
                debug!("Further processing for DataProposal");
                let kc = self.known_contracts.clone();
                let sender: &tokio::sync::broadcast::Sender<InternalMempoolEvent> = self.bus.get();
                let sender = sender.clone();
                let validator = validator.clone();
                tokio::task::spawn_blocking(move || {
                    let decision = Storage::process_data_proposal(&mut data_proposal, kc);
                    sender.send(InternalMempoolEvent::OnProcessedDataProposal((
                        validator,
                        decision,
                        data_proposal,
                    )))
                });
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

    fn on_processed_data_proposal(
        &mut self,
        validator: ValidatorPublicKey,
        verdict: DataProposalVerdict,
        data_proposal: DataProposal,
    ) -> Result<()> {
        debug!(
            "Handling processed DataProposal {:?} from {} ({} txs)",
            data_proposal.hash(),
            validator,
            data_proposal.txs.len()
        );
        let data_proposal_hash = data_proposal.hash();
        match verdict {
            DataProposalVerdict::Empty => {
                unreachable!("Empty DataProposal should never be processed");
            }
            DataProposalVerdict::Process => {
                unreachable!("DataProposal has already been processed");
            }
            DataProposalVerdict::Wait(_) => {
                unreachable!("DataProposal has already been processed");
            }
            DataProposalVerdict::Vote => {
                debug!("Send vote for DataProposal");
                self.storage
                    .store_data_proposal(&self.crypto, &validator, data_proposal);
                self.send_vote(&validator, data_proposal_hash)?;
            }
            DataProposalVerdict::Refuse => {
                debug!("Refuse vote for DataProposal");
            }
        }
        Ok(())
    }

    fn on_new_tx(&mut self, tx: Transaction) -> Result<()> {
        // TODO: Verify fees ?
        // TODO: Verify identity ?

        match tx.transaction_data {
            TransactionData::RegisterContract(ref register_contract_transaction) => {
                debug!("Got new register contract tx {}", tx.hash());

                #[allow(clippy::expect_used, reason = "not held across await")]
                self.known_contracts
                    .write()
                    .expect("logic issue")
                    .register_contract(
                        &register_contract_transaction.contract_name,
                        &register_contract_transaction.verifier,
                        &register_contract_transaction.program_id,
                    )?;
            }
            TransactionData::Blob(ref blob_tx) => {
                debug!("Got new blob tx {}", tx.hash());
                if let Err(e) = blob_tx.validate_identity() {
                    bail!("Invalid identity for blob tx {}: {}", tx.hash(), e);
                }
            }
            TransactionData::Proof(_) => {
                let kc = self.known_contracts.clone();
                let sender: &tokio::sync::broadcast::Sender<InternalMempoolEvent> = self.bus.get();
                let sender = sender.clone();
                tokio::task::spawn_blocking(move || {
                    let tx = match Self::process_proof_tx(kc, tx) {
                        Ok(tx) => tx,
                        Err(e) => bail!("Error processing proof tx: {}", e),
                    };
                    sender
                        .send(InternalMempoolEvent::OnProcessedNewTx(tx))
                        .log_warn("sending processed TX")
                });
                return Ok(());
            }
            TransactionData::VerifiedProof(ref proof_tx) => {
                debug!(
                    "Got verified proof tx {} for {}",
                    tx.hash(),
                    proof_tx.contract_name
                );
            }
        }

        let tx_type: &'static str = (&tx.transaction_data).into();

        self.metrics.add_api_tx(tx_type);
        self.storage.on_new_tx(tx);
        self.metrics
            .snapshot_pending_tx(self.storage.pending_txs.len());

        Ok(())
    }

    fn process_proof_tx(
        known_contracts: Arc<std::sync::RwLock<KnownContracts>>,
        mut tx: Transaction,
    ) -> Result<Transaction> {
        let TransactionData::Proof(proof_transaction) = tx.transaction_data else {
            bail!("Can only process ProofTx");
        };
        // Verify and extract proof
        #[allow(clippy::expect_used, reason = "not held across await")]
        let (verifier, program_id) = known_contracts
            .read()
            .expect("logic error")
            .0
            .get(&proof_transaction.contract_name)
            .context("Contract unknown")?
            .clone();

        let is_recursive = proof_transaction.contract_name.0 == "risc0-recursion";

        let (hyle_outputs, program_ids) = if is_recursive {
            let (program_ids, hyle_outputs) = verify_recursive_proof(
                &proof_transaction.proof.to_bytes()?,
                &verifier,
                &program_id,
            )?;
            (hyle_outputs, program_ids)
        } else {
            let hyle_outputs =
                verify_proof(&proof_transaction.proof.to_bytes()?, &verifier, &program_id)?;
            (hyle_outputs, vec![program_id.clone()])
        };

        let tx_hashes = hyle_outputs
            .iter()
            .map(|ho| ho.tx_hash.clone())
            .collect::<Vec<_>>();

        std::iter::zip(&tx_hashes, std::iter::zip(&hyle_outputs, &program_ids)).for_each(
            |(blob_tx_hash, (hyle_output, program_id))| {
                debug!(
                    "Blob tx hash {} verified with hyle output {:?} and program id {}",
                    blob_tx_hash,
                    hyle_output,
                    hex::encode(&program_id.0)
                );
            },
        );

        tx.transaction_data = TransactionData::VerifiedProof(VerifiedProofTransaction {
            proof_hash: proof_transaction.proof.hash(),
            proof: Some(proof_transaction.proof),
            contract_name: proof_transaction.contract_name.clone(),
            is_recursive,
            proven_blobs: std::iter::zip(tx_hashes, std::iter::zip(hyle_outputs, program_ids))
                .map(
                    |(blob_tx_hash, (hyle_output, program_id))| BlobProofOutput {
                        original_proof_hash: ProofDataHash("todo?".to_owned()),
                        blob_tx_hash,
                        hyle_output,
                        program_id,
                    },
                )
                .collect(),
        });
        debug!(
            "Got new proof tx {} for {}",
            tx.hash(),
            proof_transaction.contract_name
        );
        Ok(tx)
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

    pub fn sign_net_message(
        &self,
        msg: MempoolNetMessage,
    ) -> Result<SignedByValidator<MempoolNetMessage>> {
        self.crypto.sign(msg)
    }
}

#[cfg(test)]
pub mod test {

    mod async_data_proposals;

    use core::panic;

    use super::*;
    use crate::bus::dont_use_this::get_receiver;
    use crate::bus::metrics::BusMetrics;
    use crate::bus::SharedMessageBus;
    use crate::model::{ContractName, RegisterContractTransaction, Transaction};
    use crate::p2p::network::NetMessage;
    use crate::utils::crypto::AggregateSignature;
    use anyhow::Result;
    use assertables::assert_ok;
    use hyle_contract_sdk::StateDigest;
    use tokio::sync::broadcast::Receiver;

    pub struct MempoolTestCtx {
        pub name: String,
        pub out_receiver: Receiver<OutboundMessage>,
        pub mempool_event_receiver: Receiver<MempoolEvent>,
        pub mempool_internal_event_receiver: Receiver<InternalMempoolEvent>,
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
                da_latest_pending_cuts: VecDeque::new(),
                validators,
                known_contracts: Arc::new(std::sync::RwLock::new(KnownContracts::default())),
            }
        }

        pub async fn new(name: &str) -> Self {
            let crypto = BlstCrypto::new(name.into()).unwrap();
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));

            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let mempool_event_receiver = get_receiver::<MempoolEvent>(&shared_bus).await;
            let mempool_internal_event_receiver =
                get_receiver::<InternalMempoolEvent>(&shared_bus).await;

            let mempool = Self::build_mempool(&shared_bus, crypto).await;

            MempoolTestCtx {
                name: name.to_string(),
                out_receiver,
                mempool_event_receiver,
                mempool_internal_event_receiver,
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

        pub fn sign_data<T: bincode::Encode>(&self, data: T) -> Result<SignedByValidator<T>> {
            self.mempool.crypto.sign(data)
        }

        pub fn gen_cut(&mut self, staking: &Staking) -> Cut {
            self.mempool
                .handle_querynewcut(&mut QueryNewCut(staking.clone()))
        }

        pub fn make_data_proposal_with_pending_txs(&mut self) -> Result<()> {
            self.mempool.handle_data_proposal_management()
        }

        pub fn submit_tx(&mut self, tx: &Transaction) {
            self.mempool
                .handle_api_message(RestApiMessage::NewTx(tx.clone()))
                .expect("fail to handle new transaction");
        }

        pub async fn handle_processed_data_proposals(&mut self) {
            let event = self
                .mempool_internal_event_receiver
                .recv()
                .await
                .expect("No event received");
            self.mempool
                .handle_internal_event(event)
                .expect("fail to handle event");
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
                verifier: "test".into(),
                program_id: ProgramId(vec![]),
                state_digest: StateDigest(vec![0, 1, 2, 3]),
                contract_name: name,
            }),
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_single_mempool_receiving_new_tx() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.submit_tx(&register_tx);

        ctx.mempool.handle_data_proposal_management()?;

        assert_eq!(
            ctx.mempool
                .storage
                .lanes
                .get(&ctx.validator_pubkey())
                .unwrap()
                .data_proposals
                .first()
                .unwrap()
                .1
                .data_proposal
                .txs,
            vec![register_tx.clone()]
        );

        // Assert that pending_tx has been flushed
        assert!(ctx.mempool.storage.pending_txs.is_empty());
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_pair_mempool_receiving_new_tx() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Adding new validator
        let temp_crypto = BlstCrypto::new("validator1".into()).unwrap();
        ctx.mempool
            .validators
            .push(temp_crypto.validator_pubkey().clone());

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool.handle_data_proposal_management()?;

        let data_proposal = match ctx.assert_broadcast("DataProposal").msg {
            MempoolNetMessage::DataProposal(dp) => dp,
            _ => panic!("oops"),
        };

        let signed_msg = temp_crypto.sign(MempoolNetMessage::DataVote(data_proposal.hash()))?;
        let _ = ctx.mempool.handle_net_message(SignedByValidator {
            msg: MempoolNetMessage::DataVote(data_proposal.hash()),
            signature: signed_msg.signature,
        });

        // Adding new validator
        ctx.mempool.validators.push(ValidatorPublicKey(
            "validator2".to_owned().as_bytes().to_vec(),
        ));

        ctx.make_data_proposal_with_pending_txs()?;
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
            txs: vec![make_register_contract_tx(ContractName::new("test1"))],
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

        ctx.handle_processed_data_proposals().await;

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
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.submit_tx(&register_tx);

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![make_register_contract_tx(ContractName::new("test1"))],
        };

        let temp_crypto = BlstCrypto::new("temp_crypto".into()).unwrap();
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
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.submit_tx(&register_tx);

        let data_proposal = DataProposal {
            id: 0,
            parent_data_proposal_hash: None,
            txs: vec![make_register_contract_tx(ContractName::new("test1"))],
        };
        let data_proposal_hash = data_proposal.hash();

        ctx.make_data_proposal_with_pending_txs()?;

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into()).unwrap();
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
            2
        );
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_vote_for_unknown_data_proposal() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.submit_tx(&register_tx);

        ctx.make_data_proposal_with_pending_txs()?;

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into()).unwrap();
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
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.submit_tx(&register_tx);

        ctx.make_data_proposal_with_pending_txs()?;

        // Since mempool is alone, no broadcast

        let data_proposal = ctx
            .mempool
            .storage
            .lanes
            .get(&ctx.validator_pubkey())
            .unwrap()
            .data_proposals
            .first()
            .unwrap()
            .1
            .data_proposal
            .clone();

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into()).unwrap();
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
                assert_eq!(lane_entries.first().unwrap().data_proposal, data_proposal);
            }
            _ => panic!("Expected SyncReply message"),
        };
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_sync_reply() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.submit_tx(&register_tx);

        ctx.make_data_proposal_with_pending_txs()?;

        // Since mempool is alone, no broadcast.
        // Take this as an example data proposal.
        let data_proposal = ctx
            .mempool
            .storage
            .lanes
            .get(&ctx.validator_pubkey())
            .unwrap()
            .data_proposals
            .first()
            .unwrap()
            .1
            .data_proposal
            .clone();

        // Add new validator
        let crypto2 = BlstCrypto::new("2".into()).unwrap();
        let crypto3 = BlstCrypto::new("3".into()).unwrap();

        ctx.mempool
            .validators
            .push(crypto2.validator_pubkey().clone());

        // First: the message is from crypto2, but the DP is not signed correctly
        let signed_msg = crypto2.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            signatures: vec![crypto3
                .sign(MempoolNetMessage::DataVote(data_proposal.hash()))
                .expect("should sign")],
        }]))?;

        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_eq!(
            handle.expect_err("should fail").to_string(),
            "Empty lane entries after filtering out missing signatures"
        );

        // Second: the message is NOT from crypto2, but the DP is signed by crypto2
        let signed_msg = crypto3.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            signatures: vec![crypto2
                .sign(MempoolNetMessage::DataVote(data_proposal.hash()))
                .expect("should sign")],
        }]))?;

        // This actually fails - we don't know how to handle it
        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_eq!(
            handle.expect_err("should fail").to_string(),
            "Empty lane entries after filtering out missing signatures"
        );

        // Third: the message is from crypto2, the signature is from crypto2, but the message is wrong
        let signed_msg = crypto3.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            signatures: vec![crypto2
                .sign(MempoolNetMessage::DataVote(DataProposalHash(
                    "non_existent".to_owned(),
                )))
                .expect("should sign")],
        }]))?;

        // This actually fails - we don't know how to handle it
        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_eq!(
            handle.expect_err("should fail").to_string(),
            "Empty lane entries after filtering out missing signatures"
        );

        // Final case: message is correct
        let signed_msg = crypto2.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            signatures: vec![crypto2
                .sign(MempoolNetMessage::DataVote(data_proposal.hash()))
                .expect("should sign")],
        }]))?;

        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_ok!(handle, "Should handle net message");

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

        // Process it again
        ctx.mempool
            .handle_net_message(signed_msg)
            .expect("should ignore duplicated message");

        Ok(())
    }

    #[tokio::test]
    async fn test_fetch_data_proposals() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool.handle_data_proposal_management()?;

        let dp_orig = ctx
            .mempool
            .storage
            .lanes
            .get(&ctx.validator_pubkey())
            .unwrap()
            .data_proposals
            .first()
            .unwrap()
            .1
            .data_proposal
            .clone();
        let dp_hash = dp_orig.hash();

        let _ = ctx
            .mempool
            .handle_command(MempoolCommand::FetchDataProposals {
                from: None,
                to: vec![(
                    ctx.validator_pubkey(),
                    dp_hash.clone(),
                    AggregateSignature::default(),
                )],
            });

        let received_dp = ctx.mempool_event_receiver.try_recv().unwrap();

        match received_dp {
            MempoolEvent::DataProposals(cut, dps) => {
                assert_eq!(
                    cut,
                    vec![(
                        ctx.validator_pubkey(),
                        dp_hash,
                        AggregateSignature::default()
                    )]
                );
                assert_eq!(dps, vec![(ctx.validator_pubkey(), vec![dp_orig])]);
            }
        }

        assert!(ctx.mempool.storage.pending_txs.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_complex_fetch_data_proposals() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Sending transaction to mempool as RestApiMessage
        let register_tx = make_register_contract_tx(ContractName::new("test1"));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool.handle_data_proposal_management()?;

        let register_tx = make_register_contract_tx(ContractName::new("test2"));

        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        let register_tx = make_register_contract_tx(ContractName::new("test3"));
        ctx.mempool
            .handle_api_message(RestApiMessage::NewTx(register_tx.clone()))
            .expect("fail to handle new transaction");

        ctx.mempool.handle_data_proposal_management()?;

        let dp_1 = ctx
            .mempool
            .storage
            .lanes
            .get(&ctx.validator_pubkey())
            .unwrap()
            .data_proposals
            .first()
            .unwrap()
            .1
            .data_proposal
            .clone();
        let dp_1_hash = dp_1.hash();

        let dp_2 = ctx
            .mempool
            .storage
            .lanes
            .get(&ctx.validator_pubkey())
            .unwrap()
            .data_proposals
            .clone()
            .split_off(1)
            .first()
            .unwrap()
            .1
            .data_proposal
            .clone();
        let dp_2_hash = dp_2.hash();

        let _ = ctx
            .mempool
            .handle_command(MempoolCommand::FetchDataProposals {
                from: None,
                to: vec![(
                    ctx.validator_pubkey(),
                    dp_1_hash.clone(),
                    AggregateSignature::default(),
                )],
            });

        let _ = ctx
            .mempool
            .handle_command(MempoolCommand::FetchDataProposals {
                from: Some(vec![(
                    ctx.validator_pubkey(),
                    dp_1_hash.clone(),
                    AggregateSignature::default(),
                )]),
                to: vec![(
                    ctx.validator_pubkey(),
                    dp_2_hash.clone(),
                    AggregateSignature::default(),
                )],
            });

        let received_dp = ctx.mempool_event_receiver.try_recv().unwrap();

        match received_dp {
            MempoolEvent::DataProposals(cut, dps) => {
                assert_eq!(
                    cut,
                    vec![(
                        ctx.validator_pubkey(),
                        dp_1_hash,
                        AggregateSignature::default()
                    )]
                );
                assert_eq!(dps, vec![(ctx.validator_pubkey(), vec![dp_1.clone()])]);
                assert_eq!(dp_1.txs.len(), 1);
            }
        }
        let received_dp = ctx.mempool_event_receiver.try_recv().unwrap();

        match received_dp {
            MempoolEvent::DataProposals(cut, dps) => {
                assert_eq!(
                    cut,
                    vec![(
                        ctx.validator_pubkey(),
                        dp_2_hash,
                        AggregateSignature::default()
                    )]
                );
                assert_eq!(dps, vec![(ctx.validator_pubkey(), vec![dp_2.clone()])]);
                assert_eq!(dp_2.txs.len(), 2);
            }
        }

        assert!(ctx.mempool.storage.pending_txs.is_empty());
        Ok(())
    }
}
