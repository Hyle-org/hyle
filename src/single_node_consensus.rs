use std::path::PathBuf;
use std::sync::Arc;

use crate::bus::command_response::{CmdRespClient, Query};
use crate::bus::SharedMessageBus;
use crate::consensus::{
    ConsensusCommand, ConsensusEvent, ConsensusInfo, ConsensusNetMessage, QueryConsensusInfo,
};
use crate::mempool::storage::Cut;
use crate::mempool::{MempoolNetMessage, QueryNewCut};
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
    car_proposal_signer: SharedBlstCrypto,
    crypto: SharedBlstCrypto,
    config: SharedConf,
    store: SingleNodeConsensusStore,
    file: Option<PathBuf>,
}

/// The `SingleNodeConsensus` module listens to and sends the same messages as the `Consensus` module.
/// However, there are differences in behavior:
/// - It does not perform any consensus.
/// - It creates a fake validator that automatically votes for any `CarProposal` it receives.
/// - For every CarProposal received, it saves it automatically as a `Car`.
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
        Ok(SingleNodeConsensus {
            bus,
            car_proposal_signer: Arc::new(BlstCrypto::new(
                "single_node_car_proposal_signer".to_owned(),
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
        let validators = vec![
            self.car_proposal_signer.validator_pubkey().clone(),
            self.crypto.validator_pubkey().clone(),
        ];
        let new_bonded_validators = vec![self.crypto.validator_pubkey().clone()];

        // On peut Query DA pour récuperer le dernier block/cut ?
        if !self.store.has_done_genesis {
            // This is the genesis
            // Send new cut with two validators. The Node itself and the 'car_proposal_signer' which is here just to Vote for new CarProposal.
            self.bus.send(ConsensusEvent::CommitCut {
                validators,
                new_bonded_validators,
                cut: Cut::default(),
            })?;
            self.store.has_done_genesis = true;
            tracing::info!("Waiting for a first transaction before creating blocks...");
        }
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(
            self.config.consensus.slot_duration,
        ));

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
    }

    fn handle_car_proposal_message(&mut self, cmd: OutboundMessage) -> Result<()> {
        // Receiving a car proposal and automatically voting for it.
        // WARNING: No verification is done on it. This could be lead errors on CarProposal not being detected
        if let OutboundMessage::BroadcastMessage(NetMessage::MempoolMessage(SignedByValidator {
            msg: MempoolNetMessage::CarProposal(car_proposal),
            ..
        })) = cmd
        {
            let signed_msg =
                self.sign_net_message(MempoolNetMessage::CarProposalVote(car_proposal.clone()))?;
            let msg: SignedByValidator<MempoolNetMessage> = SignedByValidator {
                msg: MempoolNetMessage::CarProposalVote(car_proposal.clone()),
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
            self.car_proposal_signer.validator_pubkey().clone(),
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
        self.car_proposal_signer.sign(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::SharedMessageBus;
    use crate::mempool::storage::{CarId, CarProposal};
    use crate::model::ValidatorPublicKey;
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
            let car_proposal_signer = BlstCrypto::new("car_proposal_signer".to_owned());
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
                car_proposal_signer: Arc::new(car_proposal_signer),
                config: conf,
                store,
                file: None,
            };

            let mut new_cut_query_receiver = TestBusClient::new_from_bus(shared_bus).await;
            tokio::spawn(async move {
                handle_messages! {
                    on_bus new_cut_query_receiver,
                    command_response<QueryNewCut, Cut> _ => {
                        Ok(vec![(ValidatorPublicKey::default(), CarId::default())])
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
        fn assert_car_proposal_vote(&mut self, err: &str) -> CarProposal {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .signed_mempool_net_message_receiver
                .try_recv()
                .expect(format!("{err}: No message broadcasted").as_str());

            match rec.msg {
                MempoolNetMessage::CarProposalVote(car_proposal) => car_proposal,
                _ => panic!("{err}: CarProposalVote message is missing"),
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
                _ => panic!("{err}: CommitCut message is missing"),
            }
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_flow() -> Result<()> {
        let mut ctx = TestContext::new("single_node_consensus").await;

        let car_proposal = CarProposal {
            txs: vec![],
            id: CarId(1),
            parent: None,
            parent_poa: None,
        };
        let signed_msg = ctx
            .single_node_consensus
            .crypto
            .sign(MempoolNetMessage::CarProposal(car_proposal.clone()))?;

        // Sending new CarProposal to SingleNodeConsensus
        ctx.single_node_consensus.handle_car_proposal_message(
            OutboundMessage::BroadcastMessage(p2p::network::NetMessage::MempoolMessage(signed_msg)),
        )?;

        // We expect to receive a CarProposalVote for that CarProposal
        let car_proposal_vote = ctx.assert_car_proposal_vote("CarProposalVote");
        assert_eq!(car_proposal_vote.id, car_proposal.id);

        ctx.single_node_consensus.handle_new_slot_tick().await?;

        let cut = ctx.assert_commit_cut("CommitCut");
        assert!(!cut.is_empty());

        Ok(())
    }
}
