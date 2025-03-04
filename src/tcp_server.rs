use crate::{
    bus::{BusClientSender, BusMessage},
    model::SharedRunContext,
    module_handle_messages,
    utils::{
        conf::SharedConf,
        logger::LogMe,
        modules::{module_bus_client, Module},
    },
};

use anyhow::{Context, Result};
use client_sdk::tcp::{codec_tcp_server, TcpServerMessage};
use tracing::info;

impl BusMessage for TcpServerMessage {}

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
