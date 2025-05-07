pub mod p2p_server;
pub mod tcp_client;
pub mod tcp_server;

use std::{fmt::Display, sync::Arc};

use borsh::{BorshDeserialize, BorshSerialize};
use bytes::Bytes;
use sdk::hyle_model_utils::TimestampMs;
use tokio::task::JoinHandle;

use anyhow::Result;

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize, PartialEq)]
pub enum TcpMessage {
    Ping,
    Data(Arc<Vec<u8>>),
}

impl TryFrom<TcpMessage> for Bytes {
    type Error = anyhow::Error;
    fn try_from(message: TcpMessage) -> Result<Self> {
        match message {
            // This is an untagged enum, if you send exactly "PING", it'll be treated as a ping.
            TcpMessage::Ping => Ok(Bytes::from_static(b"PING")),
            TcpMessage::Data(data) => Ok(Bytes::copy_from_slice(data.as_ref())),
        }
    }
}

fn to_tcp_message(data: &impl BorshSerialize) -> Result<TcpMessage> {
    let binary = borsh::to_vec(data)?;
    Ok(TcpMessage::Data(Arc::new(binary)))
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize, PartialEq)]
pub enum P2PTcpMessage<Data: BorshDeserialize + BorshSerialize> {
    Handshake(Handshake),
    Data(Data),
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, PartialEq)]
pub enum Handshake {
    Hello(
        (
            Canal,
            sdk::SignedByValidator<NodeConnectionData>,
            TimestampMs,
        ),
    ),
    Verack(
        (
            Canal,
            sdk::SignedByValidator<NodeConnectionData>,
            TimestampMs,
        ),
    ),
}

#[derive(
    Default, Debug, Clone, BorshSerialize, BorshDeserialize, Hash, PartialEq, Eq, PartialOrd, Ord,
)]
pub struct Canal(String);

impl Canal {
    pub fn new<T: Into<String>>(t: T) -> Canal {
        Canal(t.into())
    }
}

impl Display for Canal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, PartialEq)]
pub struct NodeConnectionData {
    pub version: u16,
    pub name: String,
    pub current_height: u64,
    pub p2p_public_address: String,
    pub da_public_address: String,
    // TODO: add known peers
    // pub peers: Vec<String>, // List of known peers
}

#[derive(Debug, Clone)]
pub enum TcpEvent<Data: BorshDeserialize> {
    Message { dest: String, data: Data },
    Error { dest: String, error: String },
    Closed { dest: String },
}

/// A socket abstraction to send a receive data
#[derive(Debug)]
struct SocketStream {
    /// Last timestamp we received a ping from the peer.
    last_ping: TimestampMs,
    /// Sender to stream data to the peer
    sender: tokio::sync::mpsc::Sender<TcpMessage>,
    /// Handle to abort the sending side of the stream
    abort_sender_task: JoinHandle<()>,
    /// Handle to abort the receiving side of the stream
    abort_receiver_task: JoinHandle<()>,
}
