use crate::{
    bus::{BusClientSender, BusMessage},
    model::{SharedRunContext, Transaction},
    module_handle_messages,
    tcp::tcp_client_server,
    utils::{
        conf::SharedConf,
        logger::LogMe,
        modules::{module_bus_client, Module},
    },
};

use anyhow::{Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
pub enum TcpServerMessage {
    NewTx(Transaction),
}
#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
pub struct TcpServerResponse;
impl BusMessage for TcpServerMessage {}

tcp_client_server! {
    TcpServer,
    request: TcpServerMessage,
    response: TcpServerResponse
}

module_bus_client! {
#[derive(Debug)]
struct TcpServerBusClient {
    sender(TcpServerMessage),
}
}

#[derive(Debug)]
pub struct TcpServer {
    config: SharedConf,
    bus: TcpServerBusClient,
}

impl Module for TcpServer {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = TcpServerBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        Ok(TcpServer {
            config: ctx.common.config.clone(),
            bus,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl TcpServer {
    pub async fn start(&mut self) -> Result<()> {
        let tcp_server_address = self
            .config
            .tcp_server_address
            .as_ref()
            .context("tcp_server_address not specified in conf file. Not Starting module.")?;

        let (_, mut receiver) = codec_tcp_server::create_server(tcp_server_address.clone())
            .run_in_background()
            .await?;

        info!(
            "📡  Starting TcpServer module, listening for stream requests on {}",
            tcp_server_address
        );

        module_handle_messages! {
            on_bus self.bus,
            Some(res) = receiver.recv() => {
                _ = self.bus.send(*res.data).log_error("Sending message on TcpServerMessage topic from connection pool");
            }
        };

        Ok(())
    }
}
