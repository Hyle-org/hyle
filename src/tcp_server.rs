use std::sync::Arc;

use crate::{
    bus::BusClientSender,
    model::CommonRunContext,
    utils::{
        conf::SharedConf,
        modules::{module_bus_client, Module},
    },
};

use anyhow::Result;
use client_sdk::tcp_client::{codec_tcp_server, TcpServerMessage};
use client_sdk::{log_error, module_handle_messages};
use hyle_net::tcp::TcpEvent;
use tracing::info;

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
        let tcp_server_port = self.config.tcp_server_port;

        info!(
            "📡  Starting TcpServer module, listening for stream requests on port {}",
            &tcp_server_port
        );

        let mut server = codec_tcp_server::start_server(tcp_server_port).await?;

        module_handle_messages! {
            on_bus self.bus,
            Some(tcp_event) = server.listen_next() => {
                if let TcpEvent::Message { dest: _, data } = tcp_event {
                    _ = log_error!(self.bus.send(data), "Sending message on TcpServerMessage topic from connection pool");
                }
            }
        };

        Ok(())
    }
}
