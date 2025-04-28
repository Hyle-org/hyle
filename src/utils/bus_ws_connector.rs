use anyhow::Result;
use hyle_model::{
    api::{APIBlock, APITransaction, TransactionTypeDb},
    TransactionData,
};
use serde::Serialize;

use crate::{
    bus::{BusClientSender, SharedMessageBus},
    module_bus_client, module_handle_messages,
    modules::websocket::WsTopicMessage,
    node_state::module::NodeStateEvent,
};

use super::modules::Module;

#[derive(Debug, Clone, Serialize)]
pub enum WebsocketOutEvent {
    NodeStateEvent(NodeStateEvent),
    NewBlock(APIBlock),
    NewTx(APITransaction),
}

module_bus_client! {
#[derive(Debug)]
pub struct NodeWebsocketConnectorBusClient {
    sender(WsTopicMessage<WebsocketOutEvent>),
    receiver(NodeStateEvent),
}
}

pub struct NodeWebsocketConnector {
    bus: NodeWebsocketConnectorBusClient,
}

pub struct NodeWebsocketConnectorCtx {
    pub bus: SharedMessageBus,
}

impl Module for NodeWebsocketConnector {
    type Context = NodeWebsocketConnectorCtx;

    async fn build(ctx: Self::Context) -> Result<Self> {
        Ok(Self {
            bus: NodeWebsocketConnectorBusClient::new_from_bus(ctx.bus.new_handle()).await,
        })
    }

    async fn run(&mut self) -> Result<()> {
        module_handle_messages! {
            on_bus self.bus,
            listen<NodeStateEvent> msg => {
                self.handle_node_state_event(msg)?;
            },
        };
        Ok(())
    }
}

impl NodeWebsocketConnector {
    fn handle_node_state_event(&mut self, event: NodeStateEvent) -> Result<()> {
        self.bus.send(WsTopicMessage::new(
            "node_state",
            WebsocketOutEvent::NodeStateEvent(event.clone()),
        ))?;

        let NodeStateEvent::NewBlock(block) = event;

        let api_block = APIBlock {
            hash: block.hash.clone(),
            parent_hash: block.parent_hash,
            height: block.block_height.0,
            timestamp: block.block_timestamp.0 as i64,
        };
        self.bus.send(WsTopicMessage::new(
            "new_block",
            WebsocketOutEvent::NewBlock(api_block),
        ))?;
        for (idx, (id, tx)) in block.txs.iter().enumerate() {
            let metadata = tx.metadata(id.0.clone());
            let transaction_type = match tx.transaction_data {
                TransactionData::Blob(_) => TransactionTypeDb::BlobTransaction,
                TransactionData::Proof(_) => TransactionTypeDb::ProofTransaction,
                TransactionData::VerifiedProof(_) => TransactionTypeDb::ProofTransaction,
            };
            let transaction_status = hyle_model::api::TransactionStatusDb::Sequenced;
            let api_tx = APITransaction {
                tx_hash: metadata.id.1,
                parent_dp_hash: metadata.id.0,
                version: metadata.version,
                transaction_type,
                transaction_status,
                block_hash: Some(block.hash.clone()),
                index: Some(idx as u32),
            };
            self.bus.send(WsTopicMessage::new(
                "new_tx",
                WebsocketOutEvent::NewTx(api_tx),
            ))?;
        }
        Ok(())
    }
}
