use std::{net::Ipv4Addr, sync::Arc};

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
        let tcp_server_port = self
            .config
            .tcp_server_port
            .context("tcp_server_port not specified in conf file. Not Starting module.")?;

        info!(
            "📡  Starting TcpServer module, listening for stream requests on port {}",
            &tcp_server_port
        );

        let mut server =
            codec_tcp_server::start_server((Ipv4Addr::UNSPECIFIED, tcp_server_port)).await?;

        module_handle_messages! {
            on_bus self.bus,
            Some(res) = server.listen_next() => {
                _ = log_error!(self.bus.send(*res.data), "Sending message on TcpServerMessage topic from connection pool");
            }
        };

        Ok(())
    }
}
