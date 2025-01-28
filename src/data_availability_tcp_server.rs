//! Minimal block storage layer for data availability.

//use blocks_memory::Blocks;

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
                // let mut failed_peers = vec![];
                // for (peer, block_stream_peer) in self.stream_peer_metadata.iter_mut() {
                //     let p = block_stream_peer
                //         .sender
                //         .send(block.clone())
                //         .await
                //         .log_error(format!("Sending block {} to peer {}", block.height(), peer));

                //     if p.is_err() {
                //         failed_peers.push(peer);
                //     }
                // }
                // if failed_peers.is_empty() {
                //     Ok(())
                // } else {
                //     bail!(
                //         "Failed to send block {} to peers {:?}",
                //         block.height(),
                //         failed_peers
                //     );
                // }
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

// #[cfg(test)]
// pub mod tests {
//     #![allow(clippy::indexing_slicing)]

//     use crate::model::ValidatorPublicKey;
//     use crate::{
//         bus::BusClientSender,
//         consensus::CommittedConsensusProposal,
//         mempool::MempoolEvent,
//         model::*,
//         node_state::{
//             module::{NodeStateBusClient, NodeStateEvent},
//             NodeState,
//         },
//         utils::{conf::Conf, integration_test::find_available_port},
//     };
//     use futures::{SinkExt, StreamExt};
//     use staking::state::Staking;
//     use tokio::io::AsyncWriteExt;
//     use tokio_util::codec::{Framed, LengthDelimitedCodec};

//     use super::module_bus_client;
//     use anyhow::Result;

//     /// For use in integration tests
//     pub struct DataAvailabilityTestCtx {
//         pub node_state_bus: NodeStateBusClient,
//         pub da: super::DataAvailability,
//         pub node_state: NodeState,
//     }

//     impl DataAvailabilityTestCtx {
//         pub async fn new(shared_bus: crate::bus::SharedMessageBus) -> Self {
//             let tmpdir = tempfile::tempdir().unwrap().into_path();
//             let blocks = Blocks::new(&tmpdir).unwrap();

//             let bus = super::DABusClient::new_from_bus(shared_bus.new_handle()).await;
//             let node_state_bus = NodeStateBusClient::new_from_bus(shared_bus).await;

//             let mut config: Conf = Conf::new(None, None, None).unwrap();
//             config.da_address = format!("127.0.0.1:{}", find_available_port().await);
//             let da = super::DataAvailability {
//                 config: config.into(),
//                 bus,
//                 blocks,
//                 buffered_signed_blocks: Default::default(),
//                 stream_peer_metadata: Default::default(),
//                 need_catchup: false,
//                 catchup_task: None,
//                 catchup_height: None,
//             };

//             let node_state = NodeState::default();

//             DataAvailabilityTestCtx {
//                 node_state_bus,
//                 da,
//                 node_state,
//             }
//         }

//         pub async fn handle_signed_block(&mut self, block: SignedBlock) {
//             self.da.handle_signed_block(block.clone()).await;
//             let full_block = self.node_state.handle_signed_block(&block);
//             self.node_state_bus
//                 .send(NodeStateEvent::NewBlock(Box::new(full_block)))
//                 .unwrap();
//         }
//     }

//     #[test_log::test]
//     fn test_blocks() -> Result<()> {
//         let tmpdir = tempfile::tempdir().unwrap().into_path();
//         let mut blocks = Blocks::new(&tmpdir).unwrap();
//         let block = SignedBlock::default();
//         blocks.put(block.clone())?;
//         assert!(blocks.last().unwrap().height() == block.height());
//         let last = blocks.get(&block.hash())?;
//         assert!(last.is_some());
//         assert!(last.unwrap().height() == BlockHeight(0));
//         Ok(())
//     }

//     module_bus_client! {
//     #[derive(Debug)]
//     struct TestBusClient {
//         sender(MempoolEvent),
//     }
//     }

//     #[test_log::test(tokio::test)]
//     async fn test_da_streaming() {
//         let tmpdir = tempfile::tempdir().unwrap().into_path();
//         let blocks = Blocks::new(&tmpdir).unwrap();

//         let global_bus = crate::bus::SharedMessageBus::new(
//             crate::bus::metrics::BusMetrics::global("global".to_string()),
//         );
//         let bus = super::DABusClient::new_from_bus(global_bus.new_handle()).await;
//         let mut block_sender = TestBusClient::new_from_bus(global_bus).await;

//         let mut config: Conf = Conf::new(None, None, None).unwrap();
//         config.da_address = format!("127.0.0.1:{}", find_available_port().await);
//         let mut da = super::DataAvailability {
//             config: config.clone().into(),
//             bus,
//             blocks,
//             buffered_signed_blocks: Default::default(),
//             stream_peer_metadata: Default::default(),
//             need_catchup: false,
//             catchup_task: None,
//             catchup_height: None,
//         };

//         let mut block = SignedBlock::default();
//         let mut blocks = vec![];
//         for i in 1..15 {
//             blocks.push(block.clone());
//             block.consensus_proposal.parent_hash = block.hash();
//             block.consensus_proposal.slot = i;
//         }
//         blocks.reverse();
//         for block in blocks {
//             da.handle_signed_block(block).await;
//         }

//         tokio::spawn(async move {
//             da.start().await.unwrap();
//         });

//         // wait until it's up
//         tokio::time::sleep(std::time::Duration::from_millis(100)).await;

//         let mut stream = tokio::net::TcpStream::connect(config.da_address.clone())
//             .await
//             .unwrap();

//         // TODO: figure out why writing doesn't work with da_stream.
//         stream.write_u32(8).await.unwrap();
//         stream.write_u64(0).await.unwrap();

//         let mut da_stream = Framed::new(stream, LengthDelimitedCodec::new());

//         let mut heights_received = vec![];
//         while let Some(Ok(cmd)) = da_stream.next().await {
//             let bytes = cmd;
//             let block: SignedBlock =
//                 bincode::decode_from_slice(&bytes, bincode::config::standard())
//                     .unwrap()
//                     .0;
//             heights_received.push(block.height().0);
//             if heights_received.len() == 14 {
//                 break;
//             }
//         }
//         assert_eq!(heights_received, (0..14).collect::<Vec<u64>>());

//         da_stream.close().await.unwrap();

//         let mut ccp = CommittedConsensusProposal {
//             staking: Staking::default(),
//             consensus_proposal: ConsensusProposal::default(),
//             certificate: AggregateSignature {
//                 signature: crate::model::Signature("signature".into()),
//                 validators: vec![],
//             },
//         };

//         for i in 14..18 {
//             ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
//             ccp.consensus_proposal.slot = i;
//             block_sender
//                 .send(MempoolEvent::BuiltSignedBlock(SignedBlock {
//                     data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
//                     certificate: ccp.certificate.clone(),
//                     consensus_proposal: ccp.consensus_proposal.clone(),
//                 }))
//                 .unwrap();
//         }

//         // End of the first stream

//         let mut stream = tokio::net::TcpStream::connect(config.da_address.clone())
//             .await
//             .unwrap();

//         // TODO: figure out why writing doesn't work with da_stream.
//         stream.write_u32(8).await.unwrap();
//         stream.write_u64(0).await.unwrap();

//         let mut da_stream = Framed::new(stream, LengthDelimitedCodec::new());

//         let mut heights_received = vec![];
//         while let Some(Ok(cmd)) = da_stream.next().await {
//             let bytes = cmd;
//             let block: SignedBlock =
//                 bincode::decode_from_slice(&bytes, bincode::config::standard())
//                     .unwrap()
//                     .0;
//             dbg!(&block);
//             heights_received.push(block.height().0);
//             if heights_received.len() == 18 {
//                 break;
//             }
//         }

//         assert_eq!(heights_received, (0..18).collect::<Vec<u64>>());
//     }
// }
