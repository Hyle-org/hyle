use std::sync::Arc;

use crate::{
    bus::{BusClientSender, BusMessage},
    log_error,
    model::CommonRunContext,
    module_handle_messages,
    utils::{
        conf::SharedConf,
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
    type Context = Arc<CommonRunContext>;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = TcpServerBusClient::new_from_bus(ctx.bus.new_handle()).await;

        Ok(TcpServer {
            config: ctx.config.clone(),
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
            .tcp_address
            .as_ref()
            .context("tcp_server_address not specified in conf file. Not Starting module.")?;

        let mut server = codec_tcp_server::start_server(tcp_server_address.clone()).await?;

        info!(
            "📡  Starting TcpServer module, listening for stream requests on {}",
            tcp_server_address
        );

        module_handle_messages! {
            on_bus self.bus,
            Some(res) = server.listen_next() => {
                _ = log_error!(self.bus.send(*res.data), "Sending message on TcpServerMessage topic from connection pool");
            }
        };

        Ok(())
    }
}
