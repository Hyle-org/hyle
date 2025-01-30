//! TCP server api to catchup blocks, or get data as an indexer.

use utils::get_current_timestamp;

use crate::{
    bus::{BusClientSender, BusMessage},
    data_availability::{
        codec::{DataAvailabilityServerCodec, DataAvailabilityServerRequest},
        DataAvailabilityStreamEvent, DataAvailabilityStreamRequest,
    },
    mempool::MempoolStatusEvent,
    model::*,
    module_handle_messages,
    utils::{
        conf::SharedConf,
        logger::LogMe,
        modules::{module_bus_client, Module},
    },
};
use anyhow::{Context, Result};
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use std::collections::HashMap;
use tokio::{
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
use tokio_util::codec::Framed;
use tracing::{debug, info};

impl BusMessage for DataAvailabilityServerRequest {}

#[derive(Clone)]
pub struct QueryBlockHeight {}

module_bus_client! {
#[derive(Debug)]
struct DATcpServerBusClient {
    sender(DataAvailabilityStreamRequest),
    receiver(DataAvailabilityStreamEvent),
    receiver(MempoolStatusEvent),
}
}

/// A peer we are streaming blocks to
#[derive(Debug)]
struct BlockStreamPeer {
    /// Last timestamp we received a ping from the peer.
    last_ping: u64,
    /// Sender to stream blocks to the peer
    sender: SplitSink<Framed<TcpStream, DataAvailabilityServerCodec>, SignedBlock>,
    /// Handle to abort the receiving side of the stream
    keepalive_abort: JoinHandle<()>,
}

#[derive(Debug)]
pub struct DataAvailabilityTcpServer {
    config: SharedConf,
    bus: DATcpServerBusClient,

    // Peers subscribed to block streaming
    stream_peer_metadata: HashMap<String, BlockStreamPeer>,
}

impl Module for DataAvailabilityTcpServer {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = DATcpServerBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        Ok(DataAvailabilityTcpServer {
            config: ctx.common.config.clone(),
            bus,
            stream_peer_metadata: HashMap::new(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DataAvailabilityTcpServer {
    pub async fn start(&mut self) -> Result<()> {
        let stream_request_receiver = TcpListener::bind(&self.config.da_address).await?;
        info!(
            "📡  Starting DataAvailability TCP server module, listening for stream requests on {}",
            &self.config.da_address
        );

        let mut pending_stream_requests = JoinSet::new();

        let (ping_sender, mut ping_receiver) = tokio::sync::mpsc::channel(100);

        module_handle_messages! {
            on_bus self.bus,
            listen<MempoolStatusEvent> evt => {
                _ = self.handle_mempool_status_event(evt).await.log_error("Handling Mempool Event");
            }

            listen<DataAvailabilityStreamEvent> event => {
                _ = self.handle_da_stream_event(event).await.log_error("Handling Da server response");
            }

            // Handle new TCP connections to stream data to peers
            // We spawn an async task that waits for the start height as the first message.
            Ok((stream, addr)) = stream_request_receiver.accept() => {
                // This handler is defined inline so I don't have to give a type to pending_stream_requests
                let (sender, mut receiver) = Framed::new(stream, DataAvailabilityServerCodec::default()).split();
                // self.setup_peer(ping_sender.clone(), sender, receiver, &addr.ip().to_string());
                pending_stream_requests.spawn(async move {
                    // Read the start height from the peer.
                    match receiver.next().await {
                        Some(Ok(data)) => {
                            if let DataAvailabilityServerRequest::BlockHeight(height) = data {
                                Ok((height, sender, receiver, addr.ip().to_string()))
                            } else {
                                Err(anyhow::anyhow!("Got a ping instead of a block height"))
                            }
                        }
                        _ => Err(anyhow::anyhow!("no start height")),
                    }
                });
            }

            Some(Ok(v)) = pending_stream_requests.join_next() => {
                if let Ok((height, sender, receiver, addr)) = v.log_error("pending stream request") {
                    let _ = self.setup_peer(ping_sender.clone(), sender, receiver, &addr).await;
                    let _ = self.bus.send(DataAvailabilityStreamRequest(addr, height)).log_error("Sending data from tcp server");
                }
            }

            Some(peer_id) = ping_receiver.recv() => {
                if let Some(peer) = self.stream_peer_metadata.get_mut(&peer_id) {
                    peer.last_ping = get_current_timestamp();
                }
            }
        };

        Ok(())
    }

    async fn handle_mempool_status_event(&mut self, _event: MempoolStatusEvent) -> Result<()> {
        todo!();
    }

    async fn handle_da_stream_event(
        &mut self,
        response: DataAvailabilityStreamEvent,
    ) -> Result<()> {
        match response {
            DataAvailabilityStreamEvent::Send { peer, block } => {
                let peer_md = self
                    .stream_peer_metadata
                    .get_mut(&peer)
                    .context(format!("Getting peer {}", peer))?;
                if let Err(e) = peer_md.sender.send(block.clone()).await {
                    debug!(
                        "Couldn't send new block to peer {}, stopping streaming  : {:?}",
                        &peer, e
                    );
                    peer_md.keepalive_abort.abort();
                    self.stream_peer_metadata.remove(&peer);
                }
                Ok(())
            }
            DataAvailabilityStreamEvent::Broadcast { block } => {
                // Stream block to all peers
                // TODO: use retain once async closures are supported ?
                let mut to_remove = Vec::new();
                for (peer_id, peer) in self.stream_peer_metadata.iter_mut() {
                    let last_ping = peer.last_ping;
                    if last_ping + 60 * 5 < get_current_timestamp() {
                        info!("peer {} timed out", &peer_id);
                        peer.keepalive_abort.abort();
                        to_remove.push(peer_id.clone());
                    } else {
                        info!("streaming block {} to peer {}", block.hash(), &peer_id);
                        match peer.sender.send(block.clone()).await {
                            Ok(_) => {}
                            Err(e) => {
                                debug!(
                                    "Couldn't send new block to peer {}, stopping streaming  : {:?}",
                                    &peer_id, e
                                );
                                peer.keepalive_abort.abort();
                                to_remove.push(peer_id.clone());
                            }
                        }
                    }
                }
                for peer_id in to_remove {
                    self.stream_peer_metadata.remove(&peer_id);
                }
                Ok(())
            }
        }
    }

    async fn setup_peer(
        &mut self,
        ping_sender: tokio::sync::mpsc::Sender<String>,
        sender: SplitSink<Framed<TcpStream, DataAvailabilityServerCodec>, SignedBlock>,
        mut receiver: SplitStream<Framed<TcpStream, DataAvailabilityServerCodec>>,
        peer_ip: &String,
    ) -> Result<()> {
        // Start a task to process pings from the peer.
        // We do the processing in the main select! loop to keep things synchronous.
        // This makes it easier to store data in the same struct without mutexing.
        let peer_ip_keepalive = peer_ip.to_string();
        let keepalive_abort = tokio::task::Builder::new()
            .name("da-keep-alive-abort")
            .spawn(async move {
                loop {
                    receiver.next().await;
                    let _ = ping_sender.send(peer_ip_keepalive.clone()).await;
                }
            })?;

        // Then store data so we can send new blocks as they come.
        self.stream_peer_metadata.insert(
            peer_ip.to_string(),
            BlockStreamPeer {
                last_ping: get_current_timestamp(),
                sender,
                keepalive_abort,
            },
        );

        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    #![allow(clippy::indexing_slicing)]

    use std::collections::HashMap;

    use crate::data_availability::tests::DataAvailabilityTestCtx;
    use crate::data_availability::{DataAvailabilityStreamEvent, DataAvailabilityStreamRequest};
    use crate::model::ValidatorPublicKey;
    use crate::{
        bus::BusClientSender,
        consensus::CommittedConsensusProposal,
        mempool::MempoolEvent,
        model::*,
        utils::{conf::Conf, integration_test::find_available_port},
    };
    use staking::state::Staking;

    use super::d_a_tcp_server_bus_client::DATcpServerBusClient;
    use super::{module_bus_client, DataAvailabilityTcpServer};

    /// For use in integration tests
    pub struct DataAvailabilityTcpServerTestCtx {
        pub da_ctx: DataAvailabilityTestCtx,
        pub da_tcp: super::DataAvailabilityTcpServer,
    }

    impl DataAvailabilityTcpServerTestCtx {
        pub async fn new(shared_bus: crate::bus::SharedMessageBus) -> Self {
            let da_ctx = DataAvailabilityTestCtx::new(shared_bus.new_handle()).await;

            let mut config: Conf = Conf::new(None, None, None).unwrap();
            config.da_address = format!("127.0.0.1:{}", find_available_port().await);

            let bus = DATcpServerBusClient::new_from_bus(shared_bus).await;
            let da_tcp = DataAvailabilityTcpServer {
                config: config.into(),
                bus,
                stream_peer_metadata: HashMap::new(),
            };

            DataAvailabilityTcpServerTestCtx { da_ctx, da_tcp }
        }
    }
    module_bus_client! {
    #[derive(Debug)]
    struct TestBusClient {
        sender(MempoolEvent),
        sender(DataAvailabilityStreamRequest),
        receiver(DataAvailabilityStreamEvent),
    }
    }
    #[test_log::test(tokio::test)]
    async fn test_da_catchup() {
        let sender_global_bus = crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        );
        let mut block_sender = TestBusClient::new_from_bus(sender_global_bus.new_handle()).await;
        let mut da_tcp_sender = DataAvailabilityTcpServerTestCtx::new(sender_global_bus).await;
        let da_sender_address = da_tcp_sender.da_tcp.config.da_address.clone();

        let receiver_global_bus = crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        );
        let mut da_receiver = DataAvailabilityTestCtx::new(receiver_global_bus).await;

        // Push some blocks to the sender
        let mut block = SignedBlock::default();
        let mut blocks = vec![];
        for i in 1..11 {
            blocks.push(block.clone());
            block.consensus_proposal.parent_hash = block.hash();
            block.consensus_proposal.slot = i;
        }
        blocks.reverse();
        for block in blocks {
            da_tcp_sender.da_ctx.handle_signed_block(block).await;
        }

        tokio::spawn(async move {
            _ = futures::join!(
                da_tcp_sender.da_ctx.da.start(),
                da_tcp_sender.da_tcp.start()
            );
        });

        // wait until it's up
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Setup done
        let (tx, mut rx) = tokio::sync::mpsc::channel(200);
        da_receiver
            .ask_for_catchup_blocks(da_sender_address.clone(), tx.clone())
            .await
            .expect("Error while asking for catchup blocks");

        let mut received_blocks = vec![];
        while let Some(streamed_block) = rx.recv().await {
            da_receiver
                .handle_signed_block(streamed_block.clone())
                .await;
            received_blocks.push(streamed_block);
            if received_blocks.len() == 10 {
                break;
            }
        }
        assert_eq!(received_blocks.len(), 10);
        assert_eq!(received_blocks[0].height(), BlockHeight(0));
        assert_eq!(received_blocks[9].height(), BlockHeight(9));

        // Add a few blocks (via bus to avoid mutex)
        let mut ccp = CommittedConsensusProposal {
            staking: Staking::default(),
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature::default(),
        };

        for i in 10..15 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
            ccp.consensus_proposal.slot = i;
            block_sender
                .send(MempoolEvent::BuiltSignedBlock(SignedBlock {
                    data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
                .unwrap();
        }

        // We should still be subscribed
        while let Some(streamed_block) = rx.recv().await {
            da_receiver
                .handle_signed_block(streamed_block.clone())
                .await;
            received_blocks.push(streamed_block);
            if received_blocks.len() == 15 {
                break;
            }
        }
        assert_eq!(received_blocks.len(), 15);
        assert_eq!(received_blocks[14].height(), BlockHeight(14));

        // Unsub
        // TODO: ideally via processing the correct message
        da_receiver.take_catchup_task().unwrap().abort();

        // Add a few blocks (via bus to avoid mutex)
        let mut ccp = CommittedConsensusProposal {
            staking: Staking::default(),
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature::default(),
        };

        for i in 15..20 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
            ccp.consensus_proposal.slot = i;
            block_sender
                .send(MempoolEvent::BuiltSignedBlock(SignedBlock {
                    data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
                .unwrap();
        }

        // Resubscribe - we should only receive the new ones.
        da_receiver
            .ask_for_catchup_blocks(da_sender_address, tx)
            .await
            .expect("Error while asking for catchup blocks");

        let mut received_blocks = vec![];
        while let Some(block) = rx.recv().await {
            received_blocks.push(block);
            if received_blocks.len() == 5 {
                break;
            }
        }
        assert_eq!(received_blocks.len(), 5);
        assert_eq!(received_blocks[0].height(), BlockHeight(15));
        assert_eq!(received_blocks[4].height(), BlockHeight(19));
    }
}
