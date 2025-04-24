use crate::bus::BusMessage;
use crate::mempool::MempoolNetMessage;
use crate::model::ValidatorPublicKey;
use anyhow::Context;
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_model::{ConsensusNetMessage, SignedByValidator};
use hyle_net::tcp::P2PTcpMessage;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{self, Display};
use strum_macros::IntoStaticStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutboundMessage {
    SendMessage {
        validator_id: ValidatorPublicKey,
        msg: NetMessage,
    },
    BroadcastMessage(NetMessage),
    BroadcastMessageOnlyFor(HashSet<ValidatorPublicKey>, NetMessage),
}

impl OutboundMessage {
    pub fn broadcast<T: Into<NetMessage>>(msg: T) -> Self {
        OutboundMessage::BroadcastMessage(msg.into())
    }
    pub fn broadcast_only_for<T: Into<NetMessage>>(
        only_for: HashSet<ValidatorPublicKey>,
        msg: T,
    ) -> Self {
        OutboundMessage::BroadcastMessageOnlyFor(only_for, msg.into())
    }
    pub fn send<T: Into<NetMessage>>(validator_id: ValidatorPublicKey, msg: T) -> Self {
        OutboundMessage::SendMessage {
            validator_id,
            msg: msg.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum PeerEvent {
    NewPeer {
        name: String,
        pubkey: ValidatorPublicKey,
        da_address: String,
    },
}

impl BusMessage for PeerEvent {}
impl BusMessage for OutboundMessage {}

impl Display for NetMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let enum_variant: &'static str = self.into();
        match self {
            NetMessage::MempoolMessage(msg) => {
                _ = write!(f, "NetMessage::{} ", enum_variant);
                write!(f, "{}", msg)
            }
            NetMessage::ConsensusMessage(msg) => {
                _ = write!(f, "NetMessage::{} ", enum_variant);
                write!(f, "{}", msg)
            }
        }
    }
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    Eq,
    PartialEq,
    IntoStaticStr,
)]
#[allow(
    clippy::large_enum_variant,
    reason = "TODO: consider if we should refactor this"
)]
pub enum NetMessage {
    MempoolMessage(SignedByValidator<MempoolNetMessage>),
    ConsensusMessage(SignedByValidator<ConsensusNetMessage>),
}

hyle_net::p2p_server_mod! {
    pub consensus_mempool,
    message: super::super::NetMessage
}

impl From<NetMessage> for P2PTcpMessage<NetMessage> {
    fn from(message: NetMessage) -> Self {
        P2PTcpMessage::Data(message)
    }
}

impl From<SignedByValidator<MempoolNetMessage>> for NetMessage {
    fn from(msg: SignedByValidator<MempoolNetMessage>) -> Self {
        NetMessage::MempoolMessage(msg)
    }
}

impl From<SignedByValidator<ConsensusNetMessage>> for NetMessage {
    fn from(msg: SignedByValidator<ConsensusNetMessage>) -> Self {
        NetMessage::ConsensusMessage(msg)
    }
}

impl NetMessage {
    pub fn to_binary(&self) -> anyhow::Result<Vec<u8>> {
        borsh::to_vec(self).context("Could not serialize NetMessage")
    }
}
