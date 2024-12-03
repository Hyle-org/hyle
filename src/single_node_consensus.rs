use std::path::PathBuf;
use std::sync::Arc;

use crate::bus::command_response::{CmdRespClient, Query};
use crate::bus::SharedMessageBus;
use crate::consensus::{
    ConsensusCommand, ConsensusEvent, ConsensusInfo, ConsensusNetMessage, QueryConsensusInfo,
};
use crate::genesis::{Genesis, GenesisEvent};
use crate::mempool::storage::Cut;
use crate::mempool::{MempoolNetMessage, QueryNewCut};
use crate::model::Hashable;
use crate::p2p::network::{NetMessage, OutboundMessage, SignedByValidator};
use crate::utils::conf::SharedConf;
use crate::utils::crypto::{BlstCrypto, SharedBlstCrypto};
use crate::{
    bus::bus_client, handle_messages, mempool::MempoolEvent, model::SharedRunContext,
    utils::modules::Module,
};
use anyhow::Result;
use bincode::{Decode, Encode};

bus_client! {
struct SingleNodeConsensusBusClient {
    sender(ConsensusEvent),
    sender(GenesisEvent),
    sender(SignedByValidator<MempoolNetMessage>),
    sender(Query<QueryNewCut, Cut>),
    receiver(ConsensusCommand),
    receiver(MempoolEvent),
    receiver(MempoolNetMessage),
    receiver(SignedByValidator<ConsensusNetMessage>),
    receiver(OutboundMessage),
    receiver(Query<QueryConsensusInfo, ConsensusInfo>),
}
}

#[derive(Encode, Decode, Default)]
struct SingleNodeConsensusStore {
    has_done_genesis: bool,
    last_cut: Cut,
}

pub struct SingleNodeConsensus {
    bus: SingleNodeConsensusBusClient,
    data_proposal_signer: SharedBlstCrypto,
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
    fn name() -> &'static str {
        "SingleNodeConsensus"
    }

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
            data_proposal_signer: Arc::new(BlstCrypto::new(
                "single_node_data_proposal_signer".to_owned(),
            )),
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
        // On peut Query DA pour récuperer le dernier block/cut ?
        if !self.store.has_done_genesis {
            // This is the genesis
            let genesis_txs = Genesis::genesis_contracts_txs();

            tracing::info!("Doing genesis");
            _ = self.bus.send(GenesisEvent::GenesisBlock {
                initial_validators: vec![],
                genesis_txs,
            });
            self.store.has_done_genesis = true;
        }
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(
            self.config.consensus.slot_duration,
        ));
        interval.tick().await; // First tick is immediate

        handle_messages! {
            on_bus self.bus,
            listen<ConsensusCommand> _ => {}
            listen<SignedByValidator<ConsensusNetMessage>> _ => {}
            listen<MempoolEvent> _ => {}
            command_response<QueryConsensusInfo, ConsensusInfo> _ => {
                let slot = 0;
                let view = 0;
                let round_leader = self.crypto.validator_pubkey().clone();
                let validators = vec![];
                Ok(ConsensusInfo { slot, view, round_leader, validators })
            }
            listen<OutboundMessage> cmd => {
                self.handle_car_proposal_message(cmd)?;
            }
            _ = interval.tick() => {
                self.handle_new_slot_tick().await?;
            }
        }
        Ok(())
    }

    fn handle_car_proposal_message(&mut self, cmd: OutboundMessage) -> Result<()> {
        // Receiving a car proposal and automatically voting for it.
        // WARNING: No verification is done on it. This could be lead errors on DataProposal not being detected
        if let OutboundMessage::BroadcastMessage(NetMessage::MempoolMessage(SignedByValidator {
            msg: MempoolNetMessage::DataProposal(data_proposal),
            ..
        })) = cmd
        {
            let signed_msg =
                self.sign_net_message(MempoolNetMessage::DataVote(data_proposal.car.hash()))?;
            let msg: SignedByValidator<MempoolNetMessage> = SignedByValidator {
                msg: MempoolNetMessage::DataVote(data_proposal.car.hash()),
                signature: signed_msg.signature,
            };

            _ = self.bus.send(msg)?;

            if let Some(file) = &self.file {
                if let Err(e) = Self::save_on_disk(
                    self.config.data_directory.as_path(),
                    file.as_path(),
                    &self.store,
                ) {
                    tracing::warn!("Failed to save consensus state on disk: {}", e);
                }
            }
        }
        Ok(())
    }

    async fn handle_new_slot_tick(&mut self) -> Result<()> {
        // Query a new cut to Mempool in order to create a new CommitCut
        let validators = vec![
            self.data_proposal_signer.validator_pubkey().clone(),
            self.crypto.validator_pubkey().clone(),
        ];
        match self.bus.request(QueryNewCut(validators.clone())).await {
            Ok(cut) => {
                self.store.last_cut = cut.clone();
            }
            Err(err) => {
                // In case of an error, we reuse the last cut to avoid being considered byzantine
                tracing::error!("Error while requesting new cut: {:?}", err);
            }
        };

        _ = self.bus.send(ConsensusEvent::CommitCut {
            validators,
            cut: self.store.last_cut.clone(),
            new_bonded_validators: vec![],
        })?;
        Ok(())
    }

    fn sign_net_message(
        &self,
        msg: MempoolNetMessage,
    ) -> Result<SignedByValidator<MempoolNetMessage>> {
        self.data_proposal_signer.sign(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::SharedMessageBus;
    use crate::mempool::storage::{Car, CarHash, DataProposal};
    use crate::model::{Hashable, ValidatorPublicKey};
    use crate::p2p;
    use crate::p2p::network::SignedByValidator;
    use crate::utils::conf::Conf;
    use anyhow::Result;
    use std::sync::Arc;
    use tokio::sync::broadcast::Receiver;

    pub struct TestContext {
        signed_mempool_net_message_receiver: Receiver<SignedByValidator<MempoolNetMessage>>,
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
            let data_proposal_signer = BlstCrypto::new("data_proposal_signer".to_owned());
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let conf = Arc::new(Conf::default());
            let store = SingleNodeConsensusStore::default();

            let signed_mempool_net_message_receiver =
                get_receiver::<SignedByValidator<MempoolNetMessage>>(&shared_bus).await;
            let consensus_event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let bus = SingleNodeConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

            // Initialize Mempool
            let single_node_consensus = SingleNodeConsensus {
                bus,
                crypto: Arc::new(crypto),
                data_proposal_signer: Arc::new(data_proposal_signer),
                config: conf,
                store,
                file: None,
            };

            let mut new_cut_query_receiver = TestBusClient::new_from_bus(shared_bus).await;
            tokio::spawn(async move {
                handle_messages! {
                    on_bus new_cut_query_receiver,
                    command_response<QueryNewCut, Cut> _ => {
                        Ok(vec![(ValidatorPublicKey::default(), CarHash::default())])
                    }
                }
            });

            TestContext {
                signed_mempool_net_message_receiver,
                consensus_event_receiver,
                single_node_consensus,
            }
        }

        #[track_caller]
        fn assert_data_vote(&mut self, err: &str) -> CarHash {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .signed_mempool_net_message_receiver
                .try_recv()
                .expect(format!("{err}: No message broadcasted").as_str());

            match rec.msg {
                MempoolNetMessage::DataVote(car_hash) => car_hash,
                _ => panic!("{err}: DataVote message is missing"),
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
                ConsensusEvent::CommitCut {
                    validators: _,
                    new_bonded_validators: _,
                    cut,
                } => cut,
            }
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_flow() -> Result<()> {
        let mut ctx = TestContext::new("single_node_consensus").await;

        let data_proposal = DataProposal {
            parent_poa: None,
            car: Car {
                txs: vec![],
                parent_hash: None,
            },
        };
        let signed_msg = ctx
            .single_node_consensus
            .crypto
            .sign(MempoolNetMessage::DataProposal(data_proposal.clone()))?;

        // Sending new DataProposal to SingleNodeConsensus
        ctx.single_node_consensus.handle_car_proposal_message(
            OutboundMessage::BroadcastMessage(p2p::network::NetMessage::MempoolMessage(signed_msg)),
        )?;

        // We expect to receive a DataVote for that DataProposal
        let car_hash = ctx.assert_data_vote("DataVote");
        assert_eq!(car_hash, data_proposal.car.hash());

        ctx.single_node_consensus.handle_new_slot_tick().await?;

        let cut = ctx.assert_commit_cut("CommitCut");
        assert!(!cut.is_empty());

        Ok(())
    }
}
