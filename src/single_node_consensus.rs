use std::path::PathBuf;

use crate::bus::command_response::{CmdRespClient, Query};
use crate::bus::BusClientSender;
use crate::consensus::{
    CommittedConsensusProposal, Consensus, ConsensusEvent, ConsensusInfo, ConsensusNetMessage,
    ConsensusProposal, QueryConsensusInfo,
};
use crate::genesis::{Genesis, GenesisEvent};
use crate::mempool::storage::Cut;
use crate::mempool::QueryNewCut;
use crate::model::{get_current_timestamp_ms, Hashable};
use crate::module_handle_messages;
use crate::utils::conf::SharedConf;
use crate::utils::crypto::SharedBlstCrypto;
use crate::utils::modules::module_bus_client;
use crate::{model::SharedRunContext, utils::modules::Module};
use anyhow::Result;
use bincode::{Decode, Encode};
use staking::{Stake, Staker, Staking};
use tracing::warn;

module_bus_client! {
struct SingleNodeConsensusBusClient {
    sender(ConsensusEvent),
    sender(GenesisEvent),
    sender(Query<QueryNewCut, Cut>),
    receiver(Query<QueryConsensusInfo, ConsensusInfo>),
}
}

#[derive(Encode, Decode, Default)]
struct SingleNodeConsensusStore {
    staking: Staking,
    has_done_genesis: bool,
    consensus_proposal: ConsensusProposal,
    last_slot: u64,
    last_cut: Cut,
}

pub struct SingleNodeConsensus {
    bus: SingleNodeConsensusBusClient,
    crypto: SharedBlstCrypto,
    config: SharedConf,
    store: SingleNodeConsensusStore,
    file: Option<PathBuf>,
}

/// The `SingleNodeConsensus` module listens to and sends the same messages as the `Consensus` module.
/// However, there are differences in behavior:
/// - It does not perform any consensus.
/// - It creates a fake validator that automatically votes for any `DataProposal` it receives.
/// - For every DataProposal received, it saves it automatically as a `Car`.
/// - For every slot_duration tick, it is able to retrieve a `Car`s and create a new `CommitCut`
impl Module for SingleNodeConsensus {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let file = ctx
            .common
            .config
            .data_directory
            .clone()
            .join("consensus_single_node.bin");

        let store: SingleNodeConsensusStore = Self::load_from_disk_or_default(file.as_path());

        let bus = SingleNodeConsensusBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        let api = super::consensus::api::api(&ctx.common).await;
        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/consensus", api));
            }
        }

        Ok(SingleNodeConsensus {
            bus,
            crypto: ctx.node.crypto.clone(),
            config: ctx.common.config.clone(),
            store,
            file: Some(file),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl SingleNodeConsensus {
    async fn start(&mut self) -> Result<()> {
        let pubkey = self.crypto.validator_pubkey();
        if !self.store.staking.is_bonded(pubkey) {
            let _ = self.store.staking.add_staker(Staker {
                pubkey: pubkey.clone(),
                stake: Stake { amount: 100 },
            });
            let _ = self.store.staking.bond(pubkey.clone());
        }
        // On peut Query DA pour récuperer le dernier block/cut ?
        if !self.store.has_done_genesis {
            // This is the genesis
            let genesis_txs = Genesis::genesis_contracts_txs();

            tracing::info!("Doing genesis");

            let genesis_block =
                Consensus::genesis_block(vec![self.crypto.validator_pubkey().clone()], genesis_txs);

            _ = self.bus.send(GenesisEvent::GenesisBlock {
                block: genesis_block.clone(),
            });

            self.store.consensus_proposal = genesis_block.consensus_proposal;
            self.store.has_done_genesis = true;
        }
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(
            self.config.consensus.slot_duration,
        ));
        interval.tick().await; // First tick is immediate

        module_handle_messages! {
            on_bus self.bus,
            command_response<QueryConsensusInfo, ConsensusInfo> _ => {
                let slot = 0;
                let view = 0;
                let round_leader = self.crypto.validator_pubkey().clone();
                let validators = vec![];
                Ok(ConsensusInfo { slot, view, round_leader, validators })
            }
            _ = interval.tick() => {
                self.handle_new_slot_tick().await?;
            }
        }
        if let Some(file) = &self.file {
            if let Err(e) = Self::save_on_disk(
                self.config.data_directory.as_path(),
                file.as_path(),
                &self.store,
            ) {
                warn!(
                    "Failed to save consensus single node storage on disk: {}",
                    e
                );
            }
        }

        Ok(())
    }
    async fn handle_new_slot_tick(&mut self) -> Result<()> {
        let parent_hash = self.store.consensus_proposal.hash();

        // Query a new cut to Mempool in order to create a new CommitCut
        match self
            .bus
            .request(QueryNewCut(self.store.staking.clone()))
            .await
        {
            Ok(cut) => {
                self.store.last_cut = cut.clone();
            }
            Err(err) => {
                // In case of an error, we reuse the last cut to avoid being considered byzantine
                tracing::error!("Error while requesting new cut: {:?}", err);
            }
        };
        let new_slot = self.store.last_slot + 1;
        let current_consensus_proposal = crate::consensus::ConsensusProposal {
            slot: new_slot,
            view: 0,
            timestamp: get_current_timestamp_ms(),
            round_leader: self.crypto.validator_pubkey().clone(),
            cut: self.store.last_cut.clone(),
            new_validators_to_bond: vec![],
            parent_hash,
        };

        let current_hash = current_consensus_proposal.hash();

        let certificate = self
            .crypto
            .sign_aggregate(ConsensusNetMessage::ConfirmAck(current_hash), &[])?;

        let pubkey = self.crypto.validator_pubkey().clone();

        _ = self.bus.send(ConsensusEvent::CommitConsensusProposal(
            CommittedConsensusProposal {
                validators: vec![pubkey],
                consensus_proposal: current_consensus_proposal.clone(),
                certificate: certificate.signature,
            },
        ))?;

        self.store.last_slot = new_slot;
        self.store.consensus_proposal = current_consensus_proposal;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::dont_use_this::get_receiver;
    use crate::bus::metrics::BusMetrics;
    use crate::bus::{bus_client, SharedMessageBus};
    use crate::handle_messages;
    use crate::mempool::storage::DataProposalHash;
    use crate::model::ValidatorPublicKey;
    use crate::utils::conf::Conf;
    use crate::utils::crypto::{AggregateSignature, BlstCrypto};
    use anyhow::Result;
    use std::sync::Arc;
    use tokio::sync::broadcast::Receiver;

    pub struct TestContext {
        consensus_event_receiver: Receiver<ConsensusEvent>,
        single_node_consensus: SingleNodeConsensus,
    }

    bus_client!(
        struct TestBusClient {
            receiver(Query<QueryNewCut, Cut>),
        }
    );

    impl TestContext {
        pub async fn new(name: &str) -> Self {
            let crypto = BlstCrypto::new(name.into());
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let conf = Arc::new(Conf::default());
            let store = SingleNodeConsensusStore::default();

            let consensus_event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let bus = SingleNodeConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

            // Initialize Mempool
            let single_node_consensus = SingleNodeConsensus {
                bus,
                crypto: Arc::new(crypto),
                config: conf,
                store,
                file: None,
            };

            let mut new_cut_query_receiver = TestBusClient::new_from_bus(shared_bus).await;
            tokio::spawn(async move {
                handle_messages! {
                    on_bus new_cut_query_receiver,
                    command_response<QueryNewCut, Cut> _ => {
                        Ok(vec![(ValidatorPublicKey::default(), DataProposalHash::default(), AggregateSignature::default())])
                    }
                }
            });

            TestContext {
                consensus_event_receiver,
                single_node_consensus,
            }
        }

        #[track_caller]
        fn assert_commit_cut(&mut self, err: &str) -> Cut {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .consensus_event_receiver
                .try_recv()
                .expect(format!("{err}: No message broadcasted").as_str());

            match rec {
                ConsensusEvent::CommitConsensusProposal(CommittedConsensusProposal {
                    consensus_proposal,
                    ..
                }) => consensus_proposal.cut,
            }
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_flow() -> Result<()> {
        let mut ctx = TestContext::new("single_node_consensus").await;

        ctx.single_node_consensus.handle_new_slot_tick().await?;

        let cut = ctx.assert_commit_cut("CommitCut");
        assert!(!cut.is_empty());

        Ok(())
    }
}
