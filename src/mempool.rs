//! Mempool logic & pending transaction management.

use crate::{
    bus::{bus_client, command_response::Query, BusMessage, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{Hashable, SharedRunContext, Transaction, TransactionData},
    p2p::network::{OutboundMessage, SignedWithKey},
    rest::endpoints::RestApiMessage,
    storage::{Car, DataProposal, InMemoryStorage, Storage, TipData},
    utils::{crypto::SharedBlstCrypto, logger::LogMe, modules::Module},
};
use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use metrics::MempoolMetrics;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};
use tracing::{debug, error, info, warn};

mod metrics;

bus_client! {
struct MempoolBusClient {
    sender(OutboundMessage),
    sender(MempoolEvent),
    receiver(Query<MempoolCommand, MempoolResponse>),
    receiver(SignedWithKey<MempoolNetMessage>),
    receiver(RestApiMessage),
    receiver(ConsensusEvent),
}
}

pub struct Mempool {
    bus: MempoolBusClient,
    crypto: SharedBlstCrypto,
    metrics: MempoolMetrics,
    storage: InMemoryStorage,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
    DataProposal(DataProposal),
    DataVote(DataProposal),
    SyncRequest(DataProposal, Option<usize>),
    SyncReply(Vec<Car>),
}
impl BusMessage for MempoolNetMessage {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MempoolCommand {
    CreatePendingBatch,
}
impl BusMessage for MempoolCommand {}

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BatchInfo {
    pub tip: TipData,
    pub validator: ValidatorId,
}

impl BatchInfo {
    pub fn new(validator: ValidatorId) -> Self {
        Self {
            tip: TipData::default(),
            validator,
        }
    }
}

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize)]
pub struct Batch {
    pub info: BatchInfo,
    pub txs: Vec<Transaction>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum MempoolEvent {
    LatestBatch(Batch),
}
impl BusMessage for MempoolEvent {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MempoolResponse {
    PendingBatch { txs: Vec<Transaction> },
}
impl BusMessage for MempoolResponse {}

impl Module for Mempool {
    fn name() -> &'static str {
        "Mempool"
    }

    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = MempoolBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        let metrics = MempoolMetrics::global(ctx.common.config.id.clone());
        Ok(Mempool {
            bus,
            metrics,
            crypto: Arc::clone(&ctx.node.crypto),
            storage: InMemoryStorage::new(ctx.common.config.id.0.clone()),
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

        handle_messages! {
            on_bus self.bus,
            listen<SignedWithKey<MempoolNetMessage>> cmd => {
                self.handle_net_message(cmd).await
            }
            listen<RestApiMessage> cmd => {
                self.handle_api_message(cmd).await
            }
            listen<ConsensusEvent> cmd => {
                self.handle_event(cmd);
            }
            //listen<ValidatorRegistryNetMessage> cmd => {
            //    self.validators.handle_net_message(cmd);
            //}
        }
    }

    fn handle_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitBlock { batch_info, block } => {
                self.storage.update_lanes_after_commit(batch_info, block);
            }
        }
    }

    async fn handle_net_message(&mut self, msg: SignedWithKey<MempoolNetMessage>) {
        match BlstCrypto::verify(&msg) {
            Ok(valid) => {
                if valid {
                    match msg.msg {
                        MempoolNetMessage::NewTx(tx) => self.on_new_tx(tx).await,
                        MempoolNetMessage::DataProposal(data_proposal) => {
                            if let Err(e) = self
                                .on_data_proposal(&msg.validators.first().unwrap().0, data_proposal)
                                .await
                            {
                                error!("{:?}", e);
                            }
                        }
                        MempoolNetMessage::DataVote(data_proposal) => {
                            self.on_data_vote(&msg.validators.first().unwrap().0, data_proposal)
                                .await;
                        }
                        MempoolNetMessage::SyncRequest(data_proposal, last_index) => {
                            self.on_sync_request(
                                &msg.validators.first().unwrap().0,
                                data_proposal,
                                last_index,
                            )
                            .await;
                        }
                        MempoolNetMessage::SyncReply(cars) => {
                            if let Err(e) = self
                                .on_sync_reply(&msg.validators.first().unwrap().0, cars)
                                .await
                            {
                                error!("{:?}", e);
                            }
                        }
                    }
                } else {
                    warn!("Invalid signature for message {:?}", msg);
                }
            }
            Err(e) => warn!("Error while checking signed message: {}", e),
        }
    }

    async fn handle_api_message(&mut self, command: RestApiMessage) {
        match command {
            RestApiMessage::NewTx(tx) => {
                self.on_new_tx(tx.clone()).await;
                // hacky stuff waiting for staking contract: do not broadcast stake txs
                if !matches!(tx.transaction_data, TransactionData::Stake(_)) {
                    self.broadcast_tx(tx).await.ok();
                }
            }
        }
    }

    async fn on_sync_reply(&mut self, validator: &str, missing_cars: Vec<Car>) -> Result<()> {
        info!("{} SyncReply from sender {validator}", self.storage.id);

        info!(
            "{} adding {} missing cars to lane {validator}",
            self.storage.id,
            missing_cars.len()
        );

        self.storage.add_missing_cars(validator, missing_cars);

        let mut waiting_proposals = self.storage.get_waiting_proposals(validator);
        waiting_proposals.sort_by_key(|wp| wp.pos);

        for wp in waiting_proposals {
            if let Err(e) = self.on_data_proposal(validator, wp).await {
                error!("{:?}", e);
            }
        }
        Ok(())
    }

    async fn on_sync_request(
        &mut self,
        validator: &str,
        data_proposal: DataProposal,
        last_index: Option<usize>,
    ) {
        info!("{} SyncRequest received from sender {validator} for last_index {:?} with data proposal {} \n{}", self.storage.id, last_index, data_proposal, self.storage);

        let missing_cars = self.storage.get_missing_cars(last_index, &data_proposal);
        info!("Missing cars on {} are {:?}", validator, missing_cars);

        match missing_cars {
            None => info!("{} no missing cars", self.storage.id),
            Some(cars) => {
                if let Err(e) = self.send_sync_reply(validator, cars).await {
                    error!("{:?}", e)
                }
            }
        }
    }

    async fn on_data_vote(&mut self, validator: &str, data_proposal: DataProposal) {
        info!("Vote received from sender {}", validator);
        match self.storage.add_data_vote(validator, &data_proposal) {
            Some(_) => {
                info!("{} DataVote from {}", self.storage.id, validator)
            }
            None => {
                error!("{} unexpected DataVote from {}", self.storage.id, validator)
            }
        }
    }

    async fn on_data_proposal(
        &mut self,
        validator: &str,
        data_proposal: DataProposal,
    ) -> Result<()> {
        if self.storage.has_data_proposal(validator, &data_proposal)
            || self.storage.append_data_proposal(validator, &data_proposal)
        {
            // Normal case, we receive a proposal we already have the parent in store
            self.send_vote(validator, data_proposal).await?;
        } else {
            //We dont have the parent, so we push the data proposal in the waiting room and craft a sync demand
            self.storage
                .push_data_proposal_into_waiting_room(validator, data_proposal.clone());

            let last_available_index = self.storage.get_last_data_index(validator);

            warn!("Emitting sync request with local state {} last_available_index {:?} and data_proposal {}", self.storage, last_available_index, data_proposal);

            self.send_sync_request(validator, data_proposal, last_available_index)
                .await?;
        }
        Ok(())
    }

    async fn on_new_tx(&mut self, tx: Transaction) {
        debug!("Got new tx {}", tx.hash());
        self.metrics.add_api_tx("blob".to_string());

        match self.storage.tip_data() {
            Some((tip, txs)) => {
                self.storage.accumulate_tx(tx.clone());
                let nb_replicas = self.validators.len();
                if tip.votes.len() > nb_replicas / 3 {
                    let requests = self.storage.flush_pending_txs();
                    // Create tx chunk and broadcast it
                    let tip_id = self.storage.add_data_to_local_lane(requests.clone());

                    let data_proposal = DataProposal {
                        inner: requests,
                        pos: tip_id,
                        parent: if tip_id == 1 { None } else { Some(tip_id - 1) },
                        parent_poa: Some(tip.votes.clone()),
                    };

                    if let Err(e) = self.broadcast_data_proposal(data_proposal).await {
                        error!("{:?}", e);
                    }

                    // FIXME: push to consensus
                    // LatestBatch => validator_id + extra infos (tip_parent, tip_pos) ?
                    if let Err(e) = self
                        .bus
                        .send(MempoolEvent::LatestBatch(Batch {
                            info: BatchInfo {
                                validator: ValidatorId(self.storage.id.clone()),
                                tip,
                            },
                            txs,
                        }))
                        .context("Cannot send message over channel")
                    {
                        error!("{:?}", e);
                    }
                } else {
                    // No PoA means we rebroadcast the data proposal for non present voters
                    let mut only_for = self.validators.ids();
                    only_for.retain(|v| !tip.votes.contains(&v.0));
                    if let Err(e) = self.broadcast_data_proposal_only_for(
                        only_for,
                        DataProposal {
                            inner: txs,
                            pos: tip.pos,
                            parent: tip.parent,
                            parent_poa: None, // TODO: fetch parent votes
                        },
                    ) {
                        error!("{:?}", e);
                    }
                }
            }
            None => {
                // Genesis create a mono tx chunk and broadcast it
                let tip_id = self.storage.add_data_to_local_lane(vec![tx.clone()]);

                let data_proposal = DataProposal {
                    inner: vec![tx],
                    pos: tip_id,
                    parent: None,
                    parent_poa: None,
                };

                if let Err(e) = self.broadcast_data_proposal(data_proposal).await {
                    error!("{:?}", e)
                }
            }
        }
    }

    async fn broadcast_tx(&mut self, tx: Transaction) -> Result<()> {
        self.metrics.add_broadcasted_tx("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::broadcast(
                self.sign_net_message(MempoolNetMessage::NewTx(tx))?,
            ))
            .context("broadcasting tx")?;
        Ok(())
    }

    async fn broadcast_data_proposal(&mut self, data_proposal: DataProposal) -> Result<()> {
        self.metrics
            .add_broadcasted_data_proposal("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::broadcast(self.sign_net_message(
                MempoolNetMessage::DataProposal(data_proposal),
            )?))
            .log_error("broadcasting data_proposal");
        Ok(())
    }

    fn broadcast_data_proposal_only_for(
        &mut self,
        only_for: HashSet<ValidatorId>,
        data_proposal: DataProposal,
    ) -> Result<()> {
        self.metrics
            .add_broadcasted_data_proposal_only_for("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::broadcast_only_for(
                only_for,
                self.sign_net_message(MempoolNetMessage::DataProposal(data_proposal))?,
            ))
            .log_error("broadcasting data_proposal_only_for");
        Ok(())
    }

    async fn send_vote(&mut self, validator: &str, data_proposal: DataProposal) -> Result<()> {
        self.metrics.add_sent_data_vote("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::send(
                validator.into(),
                self.sign_net_message(MempoolNetMessage::DataVote(data_proposal))?,
            ))
            .log_error("broadcasting data_vote");
        Ok(())
    }

    async fn send_sync_request(
        &mut self,
        validator: &str,
        data_proposal: DataProposal,
        last_index: Option<usize>,
    ) -> Result<()> {
        self.metrics.add_sent_sync_request("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::send(
                validator.into(),
                self.sign_net_message(MempoolNetMessage::SyncRequest(data_proposal, last_index))?,
            ))
            .log_error("broadcasting sync_request");
        Ok(())
    }

    async fn send_sync_reply(&mut self, validator: &str, cars: Vec<Car>) -> Result<()> {
        self.metrics.add_sent_sync_reply("blob".to_string());
        _ = self
            .bus
            .send(OutboundMessage::send(
                validator.into(),
                self.sign_net_message(MempoolNetMessage::SyncReply(cars))?,
            ))
            .log_error("broadcasting sync_reply");
        Ok(())
    }

    fn sign_net_message(&self, msg: MempoolNetMessage) -> Result<SignedWithKey<MempoolNetMessage>> {
        self.crypto.sign(msg)
    }
}
