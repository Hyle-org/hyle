//! Mempool logic & pending transaction management.

use crate::{
    bus::{command_response::Query, BusClientSender, BusMessage},
    consensus::{CommittedConsensusProposal, ConsensusEvent},
    genesis::GenesisEvent,
    model::*,
    module_handle_messages,
    node_state::module::NodeStateEvent,
    p2p::network::OutboundMessage,
    utils::{
        conf::SharedConf,
        crypto::{BlstCrypto, SharedBlstCrypto},
        logger::LogMe,
        modules::{module_bus_client, Module},
        serialize::arc_rwlock_borsh,
    },
};

use anyhow::{bail, Context, Result};
use api::RestApiMessage;
use block_construction::BlockUnderConstruction;
use borsh::{BorshDeserialize, BorshSerialize};
use client_sdk::tcp::TcpServerMessage;
use hyle_contract_sdk::{ContractName, ProgramId, Verifier};
use metrics::MempoolMetrics;
use serde::{Deserialize, Serialize};
use staking::state::Staking;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fmt::Display,
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::Arc,
};
use storage::{LaneEntry, Storage};
use tokio::task::JoinSet;
use verify_tx::DataProposalVerdict;
// Pick one of the two implementations
// use storage_memory::LanesStorage;
use storage_fjall::LanesStorage;
use strum_macros::IntoStaticStr;
use tracing::{debug, info, trace, warn};

pub mod api;
pub mod block_construction;
pub mod metrics;
pub mod own_lane;
pub mod storage;
pub mod storage_fjall;
pub mod storage_memory;
pub mod verifiers;
pub mod verify_tx;

#[derive(Debug, Clone)]
pub struct QueryNewCut(pub Staking);

#[derive(Debug, Default, Clone, BorshSerialize, BorshDeserialize)]
pub struct KnownContracts(pub HashMap<ContractName, (Verifier, ProgramId)>);

impl KnownContracts {
    #[inline(always)]
    fn register_contract(
        &mut self,
        contract_name: &ContractName,
        verifier: &Verifier,
        program_id: &ProgramId,
    ) {
        debug!("🏊📝 Registering contract in mempool {:?}", contract_name);
        self.0.insert(
            contract_name.clone(),
            (verifier.clone(), program_id.clone()),
        );
    }
}

module_bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(MempoolBlockEvent),
    sender(MempoolStatusEvent),
    receiver(SignedByValidator<MempoolNetMessage>),
    receiver(RestApiMessage),
    receiver(TcpServerMessage),
    receiver(ConsensusEvent),
    receiver(GenesisEvent),
    receiver(NodeStateEvent),
    receiver(Query<QueryNewCut, Cut>),
}
}

#[derive(Default, BorshSerialize, BorshDeserialize)]
pub struct MempoolStore {
    waiting_dissemination_txs: Vec<Transaction>,
    buffered_proposals: BTreeMap<LaneId, Vec<DataProposal>>,

    // block_construction.rs
    blocks_under_contruction: VecDeque<BlockUnderConstruction>,
    buc_build_start_height: Option<u64>,

    // Common
    last_ccp: Option<CommittedConsensusProposal>,
    staking: Staking,
    #[borsh(
        serialize_with = "arc_rwlock_borsh::serialize",
        deserialize_with = "arc_rwlock_borsh::deserialize"
    )]
    known_contracts: Arc<std::sync::RwLock<KnownContracts>>,
}

pub struct Mempool {
    bus: MempoolBusClient,
    file: Option<PathBuf>,
    running_tasks: JoinSet<Result<InternalMempoolEvent>>,
    conf: SharedConf,
    crypto: SharedBlstCrypto,
    metrics: MempoolMetrics,
    lanes: LanesStorage,
    inner: MempoolStore,
}

impl Deref for Mempool {
    type Target = MempoolStore;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Mempool {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    Eq,
    PartialEq,
    IntoStaticStr,
)]
pub enum MempoolNetMessage {
    DataProposal(DataProposal),
    DataVote(DataProposalHash, LaneBytesSize), // New lane size with this DP
    PoDAUpdate(DataProposalHash, Vec<SignedByValidator<MempoolNetMessage>>),
    SyncRequest(Option<DataProposalHash>, Option<DataProposalHash>),
    SyncReply(Vec<LaneEntry>),
}

impl Display for MempoolNetMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let enum_variant: &'static str = self.into();
        write!(f, "{}", enum_variant)
    }
}

impl BusMessage for MempoolNetMessage {}
impl BusMessage for MempoolBlockEvent {}
impl BusMessage for MempoolStatusEvent {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum InternalMempoolEvent {
    OnProcessedNewTx(Transaction),
    OnProcessedDataProposal((LaneId, DataProposalVerdict, DataProposal)),
}
impl BusMessage for InternalMempoolEvent {}

impl Module for Mempool {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = MempoolBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        let metrics = MempoolMetrics::global(ctx.common.config.id.clone());

        let api = api::api(&ctx.common).await;
        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/", api));
            }
        }

        let attributes = Self::load_from_disk::<MempoolStore>(
            ctx.common
                .config
                .data_directory
                .join("mempool.bin")
                .as_path(),
        )
        .unwrap_or_default();

        let lanes_tip =
            Self::load_from_disk::<BTreeMap<LaneId, (DataProposalHash, LaneBytesSize)>>(
                ctx.common
                    .config
                    .data_directory
                    .join("mempool_lanes_tip.bin")
                    .as_path(),
            )
            .unwrap_or_default();

        // Register the Hyle contract to be able to handle registrations.
        #[allow(clippy::expect_used, reason = "not held across await")]
        attributes
            .known_contracts
            .write()
            .expect("logic issue")
            .0
            .entry("hyle".into())
            .or_insert_with(|| (Verifier("hyle".to_owned()), ProgramId(vec![])));

        Ok(Mempool {
            bus,
            file: Some(ctx.common.config.data_directory.clone()),
            conf: ctx.common.config.clone(),
            running_tasks: JoinSet::new(),
            metrics,
            crypto: Arc::clone(&ctx.node.crypto),
            lanes: LanesStorage::new(&ctx.common.config.data_directory, lanes_tip)?,
            inner: attributes,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl Mempool {
    /// start starts the mempool server.
    pub async fn start(&mut self) -> Result<()> {
        let tick_time = std::cmp::min(self.conf.consensus.slot_duration / 2, 500);
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(tick_time));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        // TODO: Recompute optimistic node_state for contract registrations.

        module_handle_messages! {
            on_bus self.bus,
            delay_shutdown_until {
                self.running_tasks.is_empty()
            },
            listen<SignedByValidator<MempoolNetMessage>> cmd => {
                let _ = self.handle_net_message(cmd)
                    .log_error("Handling MempoolNetMessage in Mempool");
            }
            listen<RestApiMessage> cmd => {
                let _ = self.handle_api_message(cmd).log_error("Handling API Message in Mempool");
            }
            listen<TcpServerMessage> cmd => {
                let _ = self.handle_tcp_server_message(cmd).log_error("Handling TCP Server message in Mempool");
            }
            listen<ConsensusEvent> cmd => {
                let _ = self.handle_consensus_event(cmd)
                    .log_error("Handling ConsensusEvent in Mempool");
            }
            listen<NodeStateEvent> cmd => {
                let NodeStateEvent::NewBlock(block) = cmd;
                for (_, contract) in block.registered_contracts {
                    self.handle_contract_registration(contract);
                }
            }
            command_response<QueryNewCut, Cut> staking => {
                self.handle_querynewcut(staking)
            }
            Some(event) = self.running_tasks.join_next() => {
                if let Ok(Ok(event)) = event.log_error("Processing InternalMempoolEvent from Blocker Joinset") {
                    let _ = self.handle_internal_event(event)
                        .log_error("Handling InternalMempoolEvent in Mempool");
                }
            }
            _ = interval.tick() => {
                let _ = self.handle_data_proposal_management()
                    .log_error("Creating Data Proposal on tick");
            }
        };

        if let Some(file) = &self.file {
            if let Err(e) = Self::save_on_disk(file.join("mempool.bin").as_path(), &self.inner) {
                warn!("Failed to save mempool storage on disk: {}", e);
            }
            if let Err(e) = Self::save_on_disk(
                file.join("mempool_lanes_tip.bin").as_path(),
                &self.lanes.lanes_tip,
            ) {
                warn!("Failed to save mempool storage on disk: {}", e);
            }
        }

        Ok(())
    }

    fn handle_contract_registration(&mut self, effect: RegisterContractEffect) {
        #[allow(clippy::expect_used, reason = "not held across await")]
        let mut known_contracts = self.known_contracts.write().expect("logic issue");
        known_contracts.register_contract(
            &effect.contract_name,
            &effect.verifier,
            &effect.program_id,
        );
    }

    // Optimistically parse Hyle tx blobs
    fn handle_hyle_contract_registration(&mut self, blob_tx: &BlobTransaction) {
        #[allow(clippy::expect_used, reason = "not held across await")]
        let mut known_contracts = self.known_contracts.write().expect("logic issue");
        blob_tx.blobs.iter().for_each(|blob| {
            if blob.contract_name.0 != "hyle" {
                return;
            }
            if let Ok(tx) =
                StructuredBlobData::<RegisterContractAction>::try_from(blob.data.clone())
            {
                let tx = tx.parameters;
                known_contracts.register_contract(&tx.contract_name, &tx.verifier, &tx.program_id);
            }
        });
    }

    /// Creates a cut with local material on QueryNewCut message reception (from consensus)
    fn handle_querynewcut(&mut self, staking: &mut QueryNewCut) -> Result<Cut> {
        self.metrics.add_new_cut(staking);
        let previous_cut = self
            .last_ccp
            .as_ref()
            .map(|ccp| ccp.consensus_proposal.cut.clone())
            .unwrap_or_default();

        let lane_last_cut: HashMap<LaneId, (DataProposalHash, PoDA)> = previous_cut
            .iter()
            .map(|(lid, dp, _, poda)| (lid.clone(), (dp.clone(), poda.clone())))
            .collect();

        // For each lane, we get the last CAR and put it in the cut
        let mut cut: Cut = vec![];
        for lane_id in self.lanes.get_lane_ids() {
            if let Some((dp_hash, cumul_size, poda)) =
                self.lanes
                    .get_latest_car(lane_id, &staking.0, lane_last_cut.get(lane_id))?
            {
                cut.push((lane_id.clone(), dp_hash, cumul_size, poda));
            }
        }
        Ok(cut)
    }

    fn handle_internal_event(&mut self, event: InternalMempoolEvent) -> Result<()> {
        match event {
            InternalMempoolEvent::OnProcessedNewTx(tx) => {
                self.on_new_tx(tx).context("Processing new tx")
            }
            InternalMempoolEvent::OnProcessedDataProposal((lane_id, verdict, data_proposal)) => {
                self.on_processed_data_proposal(lane_id, verdict, data_proposal)
                    .context("Processing data proposal")
            }
        }
    }

    fn handle_consensus_event(&mut self, event: ConsensusEvent) -> Result<()> {
        match event {
            ConsensusEvent::CommitConsensusProposal(cpp) => {
                debug!(
                    "✂️ Received CommittedConsensusProposal (slot {}, {:?} cut)",
                    cpp.consensus_proposal.slot, cpp.consensus_proposal.cut
                );

                self.staking = cpp.staking.clone();

                let cut = cpp.consensus_proposal.cut.clone();
                let previous_cut = self
                    .last_ccp
                    .as_ref()
                    .map(|ccp| ccp.consensus_proposal.cut.clone());

                self.try_create_block_under_construction(cpp);

                self.try_to_send_full_signed_blocks()?;

                // Removes all DPs that are not in the new cut, updates lane tip and sends SyncRequest for missing DPs
                self.clean_and_update_lanes(&cut, &previous_cut)?;

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
                let lane_id = self.get_lane(validator);
                self.on_data_proposal(&lane_id, data_proposal)?;
            }
            MempoolNetMessage::DataVote(ref data_proposal_hash, _) => {
                self.on_data_vote(&msg, data_proposal_hash.clone())?;
            }
            MempoolNetMessage::PoDAUpdate(data_proposal_hash, signatures) => {
                let lane_id = self.get_lane(validator);
                self.on_poda_update(&lane_id, &data_proposal_hash, signatures)?
            }
            MempoolNetMessage::SyncRequest(from_data_proposal_hash, to_data_proposal_hash) => {
                self.on_sync_request(
                    validator,
                    from_data_proposal_hash.as_ref(),
                    to_data_proposal_hash.as_ref(),
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
        sender_validator: &ValidatorPublicKey,
        missing_lane_entries: Vec<LaneEntry>,
    ) -> Result<()> {
        trace!("SyncReply from validator {sender_validator}");

        // TODO: this isn't necessarily the case - another validator could have sent us data for this lane.
        let lane_id = &LaneId(sender_validator.clone());
        let lane_operator = self.get_lane_operator(lane_id);

        // Ensure all lane entries are signed by the validator.
        if missing_lane_entries.iter().any(|lane_entry| {
            let expected_message = MempoolNetMessage::DataVote(
                lane_entry.data_proposal.hashed(),
                lane_entry.cumul_size,
            );

            !lane_entry
                .signatures
                .iter()
                .any(|s| &s.signature.validator == lane_operator && s.msg == expected_message)
        }) {
            bail!(
                "At least one lane entry is missing signature from {}",
                lane_operator
            );
        }

        // If we end up with an empty list, return an error (for testing/logic)
        if missing_lane_entries.is_empty() {
            bail!("Empty lane entries after filtering out missing signatures");
        }

        // Add missing lanes to the validator's lane
        debug!(
            "Filling hole with {} entries for {lane_id}",
            missing_lane_entries.len()
        );

        // Assert that missing lane entries are in the right order
        for window in missing_lane_entries.windows(2) {
            let first_hash = window.first().map(|le| le.data_proposal.hashed());
            let second_parent_hash = window
                .get(1)
                .and_then(|le| le.data_proposal.parent_data_proposal_hash.as_ref());
            if first_hash.as_ref() != second_parent_hash {
                bail!("Lane entries are not in the right order");
            }
        }

        // SyncReply only comes for missing data proposals. We should NEVER update the lane tip
        for lane_entry in missing_lane_entries {
            self.lanes
                .put_no_verification(lane_id.clone(), lane_entry)?;
        }

        let mut waiting_proposals = match self.buffered_proposals.get_mut(lane_id) {
            Some(waiting_proposals) => std::mem::take(waiting_proposals),
            None => vec![],
        };

        // TODO: retry remaining wp when one succeeds to be processed
        for wp in waiting_proposals.iter_mut() {
            if self.lanes.contains(lane_id, &wp.hashed()) {
                continue;
            }
            self.on_data_proposal(lane_id, std::mem::take(wp))
                .context("Consuming waiting data proposal")?;
        }

        self.try_to_send_full_signed_blocks()
            .context("Try process queued CCP")?;

        Ok(())
    }

    fn on_sync_request(
        &mut self,
        validator: &ValidatorPublicKey,
        from_data_proposal_hash: Option<&DataProposalHash>,
        to_data_proposal_hash: Option<&DataProposalHash>,
    ) -> Result<()> {
        info!(
            "{} SyncRequest received from validator {validator} for last_data_proposal_hash {:?}",
            &self.own_lane_id(),
            to_data_proposal_hash
        );

        let missing_lane_entries = self.lanes.get_lane_entries_between_hashes(
            &self.own_lane_id(),
            from_data_proposal_hash,
            to_data_proposal_hash,
        );

        match missing_lane_entries {
            Err(e) => info!(
                "Can't send sync reply as there are no missing data proposals found between {:?} and {:?} for {}: {}",
                to_data_proposal_hash, from_data_proposal_hash, self.own_lane_id(), e
            ),
            Ok(lane_entries) => {
                debug!(
                    "Missing data proposals on {} are {:?}",
                    validator, lane_entries
                );
                self.send_sync_reply(validator, lane_entries)?;
            }
        }
        Ok(())
    }

    fn on_poda_update(
        &mut self,
        lane_id: &LaneId,
        data_proposal_hash: &DataProposalHash,
        signatures: Vec<SignedByValidator<MempoolNetMessage>>,
    ) -> Result<()> {
        debug!(
            "Received {} signatures for DataProposal {} of lane {}",
            signatures.len(),
            data_proposal_hash,
            lane_id
        );
        self.lanes
            .add_signatures(lane_id, data_proposal_hash, signatures)?;
        Ok(())
    }

    fn get_lane(&self, validator: &ValidatorPublicKey) -> LaneId {
        LaneId(validator.clone())
    }

    fn get_lane_operator<'a>(&self, lane_id: &'a LaneId) -> &'a ValidatorPublicKey {
        &lane_id.0
    }

    fn send_sync_request(
        &mut self,
        lane_id: &LaneId,
        from_data_proposal_hash: Option<&DataProposalHash>,
        to_data_proposal_hash: Option<&DataProposalHash>,
    ) -> Result<()> {
        // TODO: use a more clever targeting system.
        let validator = &lane_id.0;
        debug!(
            "🔍 Sending SyncRequest to {} for DataProposal from {:?} to {:?}",
            validator, from_data_proposal_hash, to_data_proposal_hash
        );
        self.metrics
            .add_sync_request(self.crypto.validator_pubkey(), validator);
        self.send_net_message(
            validator.clone(),
            MempoolNetMessage::SyncRequest(
                from_data_proposal_hash.cloned(),
                to_data_proposal_hash.cloned(),
            ),
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
    use crate::model;
    use crate::p2p::network::NetMessage;
    use anyhow::Result;
    use assertables::assert_ok;
    use hyle_contract_sdk::StateDigest;
    use tokio::sync::broadcast::Receiver;

    pub struct MempoolTestCtx {
        pub name: String,
        pub out_receiver: Receiver<OutboundMessage>,
        pub mempool_event_receiver: Receiver<MempoolBlockEvent>,
        pub mempool_status_event_receiver: Receiver<MempoolStatusEvent>,
        pub mempool: Mempool,
    }

    impl MempoolTestCtx {
        pub async fn build_mempool(shared_bus: &SharedMessageBus, crypto: BlstCrypto) -> Mempool {
            let tmp_dir = tempfile::tempdir().unwrap().into_path();
            let lanes = LanesStorage::new(&tmp_dir, BTreeMap::default()).unwrap();
            let bus = MempoolBusClient::new_from_bus(shared_bus.new_handle()).await;

            // Initialize Mempool
            Mempool {
                bus,
                file: None,
                conf: SharedConf::default(),
                running_tasks: JoinSet::new(),
                crypto: Arc::new(crypto),
                metrics: MempoolMetrics::global("id".to_string()),
                lanes,
                inner: MempoolStore::default(),
            }
        }

        pub async fn new(name: &str) -> Self {
            let crypto = BlstCrypto::new(name).unwrap();
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));

            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let mempool_event_receiver = get_receiver::<MempoolBlockEvent>(&shared_bus).await;
            let mempool_status_event_receiver =
                get_receiver::<MempoolStatusEvent>(&shared_bus).await;

            let mempool = Self::build_mempool(&shared_bus, crypto).await;

            MempoolTestCtx {
                name: name.to_string(),
                out_receiver,
                mempool_event_receiver,
                mempool_status_event_receiver,
                mempool,
            }
        }

        pub fn setup_node(&mut self, cryptos: &[BlstCrypto]) {
            for other_crypto in cryptos.iter() {
                self.add_trusted_validator(other_crypto.validator_pubkey());
            }
        }

        pub fn own_lane(&self) -> LaneId {
            self.mempool
                .get_lane(self.mempool.crypto.validator_pubkey())
        }

        pub fn validator_pubkey(&self) -> &ValidatorPublicKey {
            self.mempool.crypto.validator_pubkey()
        }

        pub fn add_trusted_validator(&mut self, pubkey: &ValidatorPublicKey) {
            self.mempool
                .staking
                .stake(hex::encode(pubkey.0.clone()).into(), 100)
                .unwrap();

            self.mempool
                .staking
                .delegate_to(hex::encode(pubkey.0.clone()).into(), pubkey.clone())
                .unwrap();

            self.mempool
                .staking
                .bond(pubkey.clone())
                .expect("cannot bond trusted validator");
        }

        pub fn sign_data<T: borsh::BorshSerialize>(&self, data: T) -> Result<SignedByValidator<T>> {
            self.mempool.crypto.sign(data)
        }

        pub fn gen_cut(&mut self, staking: &Staking) -> Cut {
            self.mempool
                .handle_querynewcut(&mut QueryNewCut(staking.clone()))
                .unwrap()
        }

        pub fn timer_tick(&mut self) -> Result<()> {
            self.mempool.handle_data_proposal_management()
        }

        pub fn handle_poda_update(
            &mut self,
            net_message: Signed<MempoolNetMessage, ValidatorSignature>,
        ) {
            self.mempool
                .handle_net_message(net_message)
                .expect("fail to handle net message");
        }

        pub async fn handle_processed_data_proposals(&mut self) {
            let event = self
                .mempool
                .running_tasks
                .join_next()
                .await
                .expect("No event received")
                .expect("No event received")
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

        pub fn current_hash(&self, lane_id: &LaneId) -> Option<DataProposalHash> {
            self.mempool
                .lanes
                .lanes_tip
                .get(lane_id)
                .cloned()
                .map(|(h, _)| h)
        }

        #[track_caller]
        pub fn current_size_of(&self, lane_id: &LaneId) -> Option<LaneBytesSize> {
            self.mempool
                .lanes
                .lanes_tip
                .get(lane_id)
                .cloned()
                .map(|(_, s)| s)
        }

        pub fn last_lane_entry(&self, lane_id: &LaneId) -> (LaneEntry, DataProposalHash) {
            let last_dp_hash = self.current_hash(lane_id).unwrap();
            let last_dp = self
                .mempool
                .lanes
                .get_by_hash(lane_id, &last_dp_hash)
                .unwrap()
                .unwrap();

            (last_dp, last_dp_hash.clone())
        }

        pub fn current_size(&self) -> Option<LaneBytesSize> {
            let lane_id = LaneId(self.validator_pubkey().clone());
            self.current_size_of(&lane_id)
        }

        pub fn pop_data_proposal(&mut self) -> (DataProposal, DataProposalHash, LaneBytesSize) {
            let lane_id = LaneId(self.validator_pubkey().clone());
            self.pop_lane_data_proposal(&lane_id)
        }

        pub fn pop_lane_data_proposal(
            &mut self,
            lane_id: &LaneId,
        ) -> (DataProposal, DataProposalHash, LaneBytesSize) {
            // Get the latest lane entry
            let latest_data_proposal_hash = self.current_hash(lane_id).unwrap();
            let latest_lane_entry = self
                .mempool
                .lanes
                .get_by_hash(lane_id, &latest_data_proposal_hash)
                .unwrap()
                .unwrap();

            // update the tip
            if let Some(parent_dp_hash) = latest_lane_entry
                .data_proposal
                .parent_data_proposal_hash
                .as_ref()
            {
                self.mempool.lanes.lanes_tip.insert(
                    lane_id.clone(),
                    (parent_dp_hash.clone(), latest_lane_entry.cumul_size),
                );
            } else {
                self.mempool.lanes.lanes_tip.remove(lane_id);
            }

            // Remove the lane entry from db
            self.mempool
                .lanes
                .remove_lane_entry(lane_id, &latest_data_proposal_hash);

            (
                latest_lane_entry.data_proposal,
                latest_data_proposal_hash,
                latest_lane_entry.cumul_size,
            )
        }

        pub fn push_data_proposal(&mut self, dp: DataProposal) {
            let lane_id = LaneId(self.validator_pubkey().clone());

            let lane_size = self.current_size().unwrap();
            let size = lane_size + dp.estimate_size();
            self.mempool
                .lanes
                .put_no_verification(
                    lane_id,
                    LaneEntry {
                        data_proposal: dp,
                        cumul_size: size,
                        signatures: vec![],
                    },
                )
                .unwrap();
        }

        pub fn handle_consensus_event(&mut self, consensus_proposal: ConsensusProposal) {
            self.mempool
                .handle_consensus_event(ConsensusEvent::CommitConsensusProposal(
                    CommittedConsensusProposal {
                        staking: self.mempool.staking.clone(),
                        consensus_proposal,
                        certificate: AggregateSignature::default(),
                    },
                ))
                .expect("Error while handling consensus event");
        }

        pub fn submit_tx(&mut self, tx: &Transaction) {
            self.mempool
                .handle_api_message(RestApiMessage::NewTx(tx.clone()))
                .unwrap();
        }

        pub fn create_data_proposal(
            &self,
            parent_hash: Option<DataProposalHash>,
            txs: &[Transaction],
        ) -> DataProposal {
            DataProposal::new(parent_hash, txs.to_vec())
        }

        pub fn process_new_data_proposal(&mut self, dp: DataProposal) -> Result<()> {
            self.mempool.lanes.store_data_proposal(
                &self.mempool.crypto,
                &LaneId(self.mempool.crypto.validator_pubkey().clone()),
                dp,
            )?;
            Ok(())
        }

        pub fn process_cut_with_dp(
            &mut self,
            leader: &ValidatorPublicKey,
            dp_hash: &DataProposalHash,
            cumul_size: LaneBytesSize,
            slot: u64,
        ) -> Result<Cut> {
            let cut = vec![(
                LaneId(leader.clone()),
                dp_hash.clone(),
                cumul_size,
                AggregateSignature::default(),
            )];

            self.mempool
                .handle_consensus_event(ConsensusEvent::CommitConsensusProposal(
                    CommittedConsensusProposal {
                        staking: self.mempool.staking.clone(),
                        consensus_proposal: model::ConsensusProposal {
                            slot,
                            view: 0,
                            round_leader: leader.clone(),
                            cut: cut.clone(),
                            staking_actions: vec![],
                            timestamp: 777,
                            parent_hash: ConsensusProposalHash("test".to_string()),
                        },
                        certificate: AggregateSignature::default(),
                    },
                ))?;

            Ok(cut)
        }
    }

    pub fn make_register_contract_tx(name: ContractName) -> Transaction {
        BlobTransaction::new(
            "hyle.hyle",
            vec![RegisterContractAction {
                verifier: "test".into(),
                program_id: ProgramId(vec![]),
                state_digest: StateDigest(vec![0, 1, 2, 3]),
                contract_name: name,
            }
            .as_blob("hyle".into(), None, None)],
        )
        .into()
    }

    #[test_log::test(tokio::test)]
    async fn test_sending_sync_request() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;
        let crypto2 = BlstCrypto::new("2").unwrap();
        let pubkey2 = crypto2.validator_pubkey();

        ctx.handle_consensus_event(ConsensusProposal {
            cut: vec![(
                LaneId(pubkey2.clone()),
                DataProposalHash("dp_hash_in_cut".to_owned()),
                LaneBytesSize::default(),
                PoDA::default(),
            )],
            ..ConsensusProposal::default()
        });

        // Assert that we send a SyncReply
        match ctx.assert_send(crypto2.validator_pubkey(), "SyncReply").msg {
            MempoolNetMessage::SyncRequest(from, to) => {
                assert_eq!(from, None);
                assert_eq!(to, Some(DataProposalHash("dp_hash_in_cut".to_owned())));
            }
            _ => panic!("Expected SyncReply message"),
        };
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_receiving_sync_request() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        // Store the DP locally.
        let register_tx = make_register_contract_tx(ContractName::new("test1"));
        let data_proposal = ctx.create_data_proposal(None, &[register_tx.clone()]);
        ctx.process_new_data_proposal(data_proposal.clone())?;

        // Since mempool is alone, no broadcast

        let (LaneEntry { data_proposal, .. }, _) =
            ctx.last_lane_entry(&LaneId(ctx.validator_pubkey().clone()));

        // Add new validator
        let crypto2 = BlstCrypto::new("2").unwrap();
        ctx.add_trusted_validator(crypto2.validator_pubkey());

        let signed_msg = crypto2.sign(MempoolNetMessage::SyncRequest(
            None,
            Some(data_proposal.hashed()),
        ))?;

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

        // Create a DP and simulate we requested it.
        let register_tx = make_register_contract_tx(ContractName::new("test1"));
        let data_proposal = ctx.create_data_proposal(None, &[register_tx.clone()]);
        let cumul_size = LaneBytesSize(data_proposal.estimate_size() as u64);

        // Add new validator
        let crypto2 = BlstCrypto::new("2").unwrap();
        let crypto3 = BlstCrypto::new("3").unwrap();

        ctx.add_trusted_validator(crypto2.validator_pubkey());

        // First: the message is from crypto2, but the DP is not signed correctly
        let signed_msg = crypto2.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            cumul_size,
            signatures: vec![crypto3
                .sign(MempoolNetMessage::DataVote(
                    data_proposal.hashed(),
                    cumul_size,
                ))
                .expect("should sign")],
        }]))?;

        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_eq!(
            handle.expect_err("should fail").to_string(),
            format!(
                "At least one lane entry is missing signature from {}",
                crypto2.validator_pubkey()
            )
        );

        // Second: the message is NOT from crypto2, but the DP is signed by crypto2
        let signed_msg = crypto3.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            cumul_size,
            signatures: vec![crypto2
                .sign(MempoolNetMessage::DataVote(
                    data_proposal.hashed(),
                    cumul_size,
                ))
                .expect("should sign")],
        }]))?;

        // This actually fails - we don't know how to handle it
        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_eq!(
            handle.expect_err("should fail").to_string(),
            format!(
                "At least one lane entry is missing signature from {}",
                crypto3.validator_pubkey()
            )
        );

        // Third: the message is from crypto2, the signature is from crypto2, but the message is wrong
        let signed_msg = crypto3.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            cumul_size,
            signatures: vec![crypto2
                .sign(MempoolNetMessage::DataVote(
                    DataProposalHash("non_existent".to_owned()),
                    cumul_size,
                ))
                .expect("should sign")],
        }]))?;

        // This actually fails - we don't know how to handle it
        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_eq!(
            handle.expect_err("should fail").to_string(),
            format!(
                "At least one lane entry is missing signature from {}",
                crypto3.validator_pubkey()
            )
        );

        // Fourth: the message is from crypto2, the signature is from crypto2, but the size is wrong
        let signed_msg = crypto2.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            cumul_size: LaneBytesSize(0),
            signatures: vec![crypto2
                .sign(MempoolNetMessage::DataVote(
                    data_proposal.hashed(),
                    cumul_size,
                ))
                .expect("should sign")],
        }]))?;

        // This actually fails - we don't know how to handle it
        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_eq!(
            handle.expect_err("should fail").to_string(),
            format!(
                "At least one lane entry is missing signature from {}",
                crypto2.validator_pubkey()
            )
        );

        // Fifth case: DP chaining is incorrect
        let incorrect_parent = DataProposal::new(
            Some(DataProposalHash("incorrect".to_owned())),
            vec![make_register_contract_tx(ContractName::new("test1"))],
        );
        let signed_msg = crypto2.sign(MempoolNetMessage::SyncReply(vec![
            LaneEntry {
                data_proposal: data_proposal.clone(),
                cumul_size,
                signatures: vec![crypto2
                    .sign(MempoolNetMessage::DataVote(
                        data_proposal.hashed(),
                        cumul_size,
                    ))
                    .expect("should sign")],
            },
            LaneEntry {
                data_proposal: incorrect_parent.clone(),
                cumul_size,
                signatures: vec![crypto2
                    .sign(MempoolNetMessage::DataVote(
                        incorrect_parent.hashed(),
                        cumul_size,
                    ))
                    .expect("should sign")],
            },
        ]))?;

        // This actually fails - we don't know how to handle it
        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_eq!(
            handle.expect_err("should fail").to_string(),
            "Lane entries are not in the right order"
        );
        // Final case: message is correct
        let signed_msg = crypto2.sign(MempoolNetMessage::SyncReply(vec![LaneEntry {
            data_proposal: data_proposal.clone(),
            cumul_size,
            signatures: vec![crypto2
                .sign(MempoolNetMessage::DataVote(
                    data_proposal.hashed(),
                    cumul_size,
                ))
                .expect("should sign")],
        }]))?;

        let handle = ctx.mempool.handle_net_message(signed_msg.clone());
        assert_ok!(handle, "Should handle net message");

        // Assert that the lane entry was added
        assert!(ctx.mempool.lanes.contains(
            &ctx.mempool.get_lane(crypto2.validator_pubkey()),
            &data_proposal.hashed()
        ));

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_serialization_deserialization() -> Result<()> {
        let mut ctx = MempoolTestCtx::new("mempool").await;
        ctx.mempool.file = Some(".".into());

        assert!(Mempool::save_on_disk(
            ctx.mempool
                .file
                .clone()
                .unwrap()
                .join("test-mempool.bin")
                .as_path(),
            &ctx.mempool.inner
        )
        .is_ok());

        assert!(Mempool::load_from_disk::<MempoolStore>(
            ctx.mempool.file.unwrap().join("test-mempool.bin").as_path(),
        )
        .is_some());

        std::fs::remove_file("./test-mempool.bin").expect("Failed to delete test-mempool.bin");

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_get_latest_car_and_new_cut() {
        let mut ctx = MempoolTestCtx::new("mempool").await;

        let dp_orig = ctx.create_data_proposal(None, &[]);
        ctx.process_new_data_proposal(dp_orig.clone()).unwrap();

        let staking = Staking::new();

        let latest = ctx
            .mempool
            .lanes
            .get_latest_car(&ctx.own_lane(), &staking, None)
            .unwrap();
        assert!(latest.is_none());

        // Force some signature for f+1 check if needed:
        // This requires more advanced stubbing of Staking if you want a real test.

        let cut = ctx
            .mempool
            .handle_querynewcut(&mut QueryNewCut(staking))
            .unwrap();
        assert_eq!(0, cut.len());
    }
}
