use crate::{
    bus::BusMessage,
    model::{SharedRunContext, Transaction},
    module_handle_messages,
    p2p::stream::read_stream,
    utils::{
        conf::SharedConf,
        modules::{module_bus_client, Module},
    },
};

use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use hyle_model::TcpServerNetMessage;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, net::TcpListener};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::info;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum TcpServerMessage {
    NewTx(Transaction),
}
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

        let tcp_listener = TcpListener::bind(tcp_server_address).await?;

        info!(
            "📡  Starting TcpServer module, listening for stream requests on {}",
            tcp_server_address
        );

        let mut readers: tokio::task::JoinSet<Result<()>> = tokio::task::JoinSet::new();

        module_handle_messages! {
            on_bus self.bus,

            Ok((tcp_stream, _)) = tcp_listener.accept() => {
                let sender: &tokio::sync::broadcast::Sender<TcpServerMessage> = self.bus.get();
                let sender = sender.clone();
                readers.spawn(async move {
                    let mut framed = Framed::new(tcp_stream, LengthDelimitedCodec::new());
                    loop {
                        let net_msg = read_stream(&mut framed).await.context("Reading TCP stream")?;
                        match net_msg {
                            TcpServerNetMessage::NewTx(tx) => {
                                sender.send(TcpServerMessage::NewTx(tx))?;
                            },
                            TcpServerNetMessage::Ping => {
                                framed.get_mut().write_all(b"Pong").await?;
                            }
                        };
                    }
                });
            }
        };

        readers.abort_all();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bincode::encode_to_vec;
    use futures::SinkExt;
    use rand::Rng;
    use std::{sync::Arc, time::Duration};
    use tokio::{io::AsyncReadExt, net::TcpStream, sync::broadcast::Receiver, time::timeout};
    use tokio_util::codec::FramedWrite;

    use crate::{
        bus::{dont_use_this::get_receiver, SharedMessageBus},
        model::RegisterContractTransaction,
        utils::conf::Conf,
    };

    use super::*;

    pub async fn build() -> (TcpServer, Receiver<TcpServerMessage>) {
        let shared_bus = SharedMessageBus::default();

        let bus = TcpServerBusClient::new_from_bus(shared_bus.new_handle()).await;

        let tcp_server_message_receiver = get_receiver::<TcpServerMessage>(&shared_bus).await;

        let mut rng = rand::thread_rng();
        let random_port: u32 = rng.gen_range(1024..65536);

        let config = Conf {
            run_tcp_server: true,
            tcp_server_address: Some(format!("127.0.0.1:{random_port}").to_string()),
            ..Default::default()
        };

        (
            TcpServer {
                config: Arc::new(config),
                bus,
            },
            tcp_server_message_receiver,
        )
    }

    pub async fn assert_server_up(addr: &str, timeout_duration: u64) -> Result<()> {
        let mut connected = false;

        timeout(Duration::from_millis(timeout_duration), async {
            loop {
                match TcpStream::connect(addr).await {
                    Ok(_) => {
                        connected = true;
                        break;
                    }
                    _ => {
                        info!("⏰ Waiting for server to be ready");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Timeout reached while waiting for height: {e}"))?;

        assert!(
            connected,
            "Could not connect after {timeout_duration} seconds"
        );

        info!("✅ Server is ready");
        Ok(())
    }

    pub async fn assert_new_tx(
        mut receiver: Receiver<TcpServerMessage>,
        tx: Transaction,
        timeout_duration: u64,
    ) -> Result<Transaction> {
        timeout(Duration::from_millis(timeout_duration), async {
            loop {
                match receiver.try_recv() {
                    Ok(TcpServerMessage::NewTx(received_tx)) => {
                        if tx == received_tx {
                            return Ok(tx);
                        } else {
                            println!("Expected NewTx({:?}), found NewTx({:?})", tx, received_tx);
                        }
                    }
                    Err(_) => {
                        info!("⏰ Waiting for server to be send transaction message");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Timeout reached while waiting for new transaction: {e}"))?
    }

    #[test_log::test(tokio::test)]
    async fn test_tcp_server() -> Result<()> {
        let (mut tcp_server, _) = build().await;

        let addr = tcp_server.config.tcp_server_address.clone().unwrap();

        // Starts server
        tokio::spawn(async move {
            let result = tcp_server.start().await;
            assert!(result.is_ok(), "{}", result.unwrap_err().to_string());
        });

        assert_server_up(&addr, 500).await?;
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_ping() -> Result<()> {
        let (mut tcp_server, _) = build().await;

        let addr = tcp_server.config.tcp_server_address.clone().unwrap();

        // Starts server
        tokio::spawn(async move {
            let result = tcp_server.start().await;
            assert!(result.is_ok(), "{}", result.unwrap_err().to_string());
        });

        assert_server_up(&addr, 500).await?;

        let stream = TcpStream::connect(addr).await?;
        let mut framed = FramedWrite::new(stream, LengthDelimitedCodec::new());

        let encoded_msg = encode_to_vec(&TcpServerNetMessage::Ping, bincode::config::standard())?;

        framed.send(encoded_msg.into()).await?;

        // Reading the pong response
        let mut buf = vec![0; 4];
        framed.get_mut().read_exact(&mut buf).await?;
        assert_eq!(&buf, b"Pong");

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_send_transaction() -> Result<()> {
        let (mut tcp_server, tcp_message_receiver) = build().await;

        let addr = tcp_server.config.tcp_server_address.clone().unwrap();

        // Starts server
        tokio::spawn(async move {
            let result = tcp_server.start().await;
            assert!(result.is_ok(), "{}", result.unwrap_err().to_string());
        });

        // wait until it's up
        assert_server_up(&addr, 500).await?;

        // Sending the transaction
        let stream = TcpStream::connect(addr).await?;
        let mut framed = FramedWrite::new(stream, LengthDelimitedCodec::new());

        let tx: Transaction = RegisterContractTransaction::default().into();
        let net_msg = TcpServerNetMessage::NewTx(tx.clone());
        framed.send(net_msg.to_binary().unwrap().into()).await?;

        assert_new_tx(tcp_message_receiver, tx, 500).await?;

        Ok(())
    }
}
