use crate::{
    bus::{BusClientSender, BusMessage},
    model::{Hashable, SharedRunContext, Transaction},
    module_handle_messages,
    p2p::{network::NetMessage, stream::read_stream},
    utils::{
        conf::SharedConf,
        modules::{module_bus_client, Module},
    },
};

use anyhow::Result;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, net::TcpListener};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{info, warn};

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
        let tcp_listener = TcpListener::bind(&self.config.tcp_server_address).await?;

        info!(
            "📡  Starting TcpServer module, listening for stream requests on {}",
            &self.config.tcp_server_address
        );

        module_handle_messages! {
            on_bus self.bus,

            Ok((tcp_stream, _)) = tcp_listener.accept() => {
                let mut framed = Framed::new(tcp_stream, LengthDelimitedCodec::new());
                match read_stream(&mut framed).await {
                    Ok(NetMessage::NewTx(tx)) => {
                        let tx_hash = tx.hash();
                        self.bus.send(TcpServerMessage::NewTx(tx))?;
                        framed.get_mut().write_all(tx_hash.0.as_bytes()).await?;
                    },
                    Ok(NetMessage::Ping) => {
                        framed.get_mut().write_all(b"Pong").await?;
                    },
                    Err(e) => { warn!("Error reading stream: {}", e) }
                };
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bincode::encode_to_vec;
    use futures::SinkExt;
    use std::{sync::Arc, time::Duration};
    use tokio::{io::AsyncReadExt, net::TcpStream, sync::broadcast::Receiver, time::timeout};
    use tokio_util::codec::FramedWrite;

    use crate::{
        bus::{dont_use_this::get_receiver, SharedMessageBus},
        model::{RegisterContractTransaction, TransactionData},
        utils::conf::Conf,
    };

    use super::*;

    pub async fn build() -> (TcpServer, Receiver<TcpServerMessage>) {
        let shared_bus = SharedMessageBus::default();

        let bus = TcpServerBusClient::new_from_bus(shared_bus.new_handle()).await;

        let tcp_server_message_receiver = get_receiver::<TcpServerMessage>(&shared_bus).await;

        let config = Conf {
            tcp_server_address: "127.0.0.1:12345".to_string(),
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

        let addr = tcp_server.config.tcp_server_address.clone();

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

        let addr = tcp_server.config.tcp_server_address.clone();

        // Starts server
        tokio::spawn(async move {
            let result = tcp_server.start().await;
            assert!(result.is_ok(), "{}", result.unwrap_err().to_string());
        });

        assert_server_up(&addr, 500).await?;

        let stream = TcpStream::connect(addr).await?;
        let mut framed = FramedWrite::new(stream, LengthDelimitedCodec::new());

        let encoded_msg = encode_to_vec(&NetMessage::Ping, bincode::config::standard())?;

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

        let addr = tcp_server.config.tcp_server_address.clone();

        // Starts server
        tokio::spawn(async move {
            let result = tcp_server.start().await;
            assert!(result.is_ok(), "{}", result.unwrap_err().to_string());
        });

        let tx = Transaction::wrap(TransactionData::RegisterContract(
            RegisterContractTransaction::default(),
        ));

        let net_msg = NetMessage::NewTx(tx.clone());
        let encoded_msg = encode_to_vec(&net_msg, bincode::config::standard())?;

        // wait until it's up
        assert_server_up(&addr, 500).await?;

        // Sending the transaction
        let stream = TcpStream::connect(addr).await?;
        let mut framed = FramedWrite::new(stream, LengthDelimitedCodec::new());

        framed.send(encoded_msg.into()).await?;

        assert_new_tx(tcp_message_receiver, tx, 500).await?;

        Ok(())
    }
}
