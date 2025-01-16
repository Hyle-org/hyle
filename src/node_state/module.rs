//! State required for participation in consensus by the node.

use super::NodeState;
use crate::bus::{command_response::Query, BusClientSender, BusMessage};
use crate::data_availability::{DataEvent, QueryBlockHeight};
use crate::model::Contract;
use crate::model::{Block, BlockHeight, CommonRunContext, ContractName};
use crate::module_handle_messages;
use crate::utils::conf::SharedConf;
use crate::utils::logger::LogMe;
use crate::utils::modules::{module_bus_client, Module};
use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

/// NodeStateModule maintains a NodeState,
/// listens to DA, and sends events when it has processed blocks.
/// Node state module is separate from DataAvailabiliity
/// mostly to run asynchronously.
pub struct NodeStateModule {
    config: SharedConf,
    bus: NodeStateBusClient,
    inner: NodeState,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NodeStateEvent {
    NewBlock(Box<Block>),
}
impl BusMessage for NodeStateEvent {}

module_bus_client! {
#[derive(Debug)]
pub struct NodeStateBusClient {
    sender(NodeStateEvent),
    receiver(DataEvent),
    receiver(Query<ContractName, Contract>),
    receiver(Query<QueryBlockHeight , BlockHeight>),
}
}

impl Module for NodeStateModule {
    type Context = Arc<CommonRunContext>;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = NodeStateBusClient::new_from_bus(ctx.bus.new_handle()).await;

        let api = super::api::api(&ctx).await;
        if let Ok(mut guard) = ctx.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/", api));
            }
        }

        let storage = Self::load_from_disk_or_default::<NodeState>(
            ctx.config.data_directory.join("node_state.bin").as_path(),
        );

        for name in storage.contracts.keys() {
            info!("📝 Loaded contract state for {}", name);
        }

        Ok(Self {
            config: ctx.config.clone(),
            bus,
            inner: storage,
        })
    }

    async fn run(&mut self) -> Result<()> {
        module_handle_messages! {
            on_bus self.bus,
            command_response<QueryBlockHeight, BlockHeight> _ => {
                Ok(self.inner.current_height)
            }
            command_response<ContractName, Contract> cmd => {
                self.inner.contracts.get(cmd).cloned().context("Contract not found")
            }
            listen<DataEvent> block => {
                match block {
                    DataEvent::OrderedSignedBlock(block) => {
                        let node_state_block = self.inner.handle_signed_block(&block);
                        _ = self
                            .bus
                            .send(NodeStateEvent::NewBlock(Box::new(node_state_block)))
                            .log_error("Sending DataEvent while processing SignedBlock");
                    }
                }
            }
        };

        let _ = Self::save_on_disk::<NodeState>(
            self.config.data_directory.join("node_state.bin").as_path(),
            &self.inner,
        )
        .log_error("Saving node state");

        Ok(())
    }
}
