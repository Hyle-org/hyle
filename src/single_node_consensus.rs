use std::path::PathBuf;
use std::sync::Arc;

use crate::bus::command_response::Query;
use crate::bus::SharedMessageBus;
use crate::consensus::{
    ConsensusCommand, ConsensusEvent, ConsensusInfo, ConsensusNetMessage, QueryConsensusInfo,
};
use crate::mempool::storage::{CarId, Cut};
use crate::mempool::MempoolNetMessage;
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
    latest_car_id: Option<CarId>,
}

pub struct SingleNodeConsensus {
    bus: SingleNodeConsensusBusClient,
    car_proposal_signer: SharedBlstCrypto,
    crypto: SharedBlstCrypto,
    store: SingleNodeConsensusStore,
    file: Option<PathBuf>,
    config: SharedConf,
}

/// The `SingleNodeConsensus` module listens to and sends the same messages as the `Consensus` module.
/// However, there are differences in behavior:
/// - It does not perform any consensus.
/// - It creates a fake validator that automatically votes for any `CarProposal` it receives.
/// - For every CarProposal received, it saves it automatically as a `Car`.
/// - When Mempool sends a new `Cut`, it is able to retrieve the associated `Car`s and create a new `CommitCut`
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
            store,
            file: Some(file),
            config: ctx.common.config.clone(),
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

        if self.store.latest_car_id.is_none() {
            // This is the genesis
            // Send new cut with two validators. The Node itself and the 'car_proposal_signer' which is here just to Vote for new CarProposal.
            self.bus.send(ConsensusEvent::CommitCut {
                validators,
                new_bonded_validators,
                cut: Cut::default(),
            })?;
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
                // Receiving a car proposal and automatically voting for it.
                // WARNING: No verification is done on it. This could be lead errors on CarProposal not being detected
                if let OutboundMessage::BroadcastMessage(NetMessage::MempoolMessage(SignedByValidator { msg: MempoolNetMessage::CarProposal(car_proposal), .. })) = cmd {
                    let signed_msg = self.sign_net_message(MempoolNetMessage::CarProposalVote(car_proposal.clone()))?;
                    let msg: SignedByValidator<MempoolNetMessage> = SignedByValidator {
                        msg: MempoolNetMessage::CarProposalVote(car_proposal.clone()),
                        signature: signed_msg.signature,
                    };
                    _ = self.bus.send(msg)?;

                    if let Some(file) = &self.file {
                        if let Err(e) = Self::save_on_disk(file.as_path(), &self.store) {
                            tracing::warn!("Failed to save consensus state on disk: {}", e);
                        }
                    }

                    // Add CarProposal as a latest car received
                    self.store.latest_car_id = Some(car_proposal.id);
                }
            }
            _ = interval.tick() => {
                tracing::error!("Tick... {:?}", self.store.latest_car_id);
                if let Some(latest_car_id) = self.store.latest_car_id.take() {
                    _ = self.bus
                        .send(ConsensusEvent::CommitCut {
                            validators: vec![
                                self.car_proposal_signer.validator_pubkey().clone(),
                                self.crypto.validator_pubkey().clone(),
                            ],
                            cut: vec![(self.crypto.validator_pubkey().clone(), latest_car_id)],
                            new_bonded_validators: vec![],
                        })?;
                }
            }
        }
    }

    fn sign_net_message(
        &self,
        msg: MempoolNetMessage,
    ) -> Result<SignedByValidator<MempoolNetMessage>> {
        self.car_proposal_signer.sign(msg)
    }
}
