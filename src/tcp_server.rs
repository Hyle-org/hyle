use crate::bus::BusClientSender;

use anyhow::Result;
use client_sdk::tcp_client::{TcpApiServer, TcpServerMessage};
use hyle_modules::{
    bus::SharedMessageBus,
    log_error, module_handle_messages,
    modules::{module_bus_client, Module},
};
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
    tcp_server_port: u16,
    bus: TcpServerBusClient,
}

impl Module for TcpServer {
    type Context = u16;

    async fn build(bus: SharedMessageBus, ctx: Self::Context) -> Result<Self> {
        let bus = TcpServerBusClient::new_from_bus(bus.new_handle()).await;

        Ok(TcpServer {
            tcp_server_port: ctx,
            bus,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl TcpServer {
    pub async fn start(&mut self) -> Result<()> {
        let tcp_server_port = self.tcp_server_port;

        info!(
            "📡  Starting TcpServer module, listening for stream requests on port {}",
            &tcp_server_port
        );

        let mut server = TcpApiServer::start(tcp_server_port, "TcpApiServer").await?;

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
