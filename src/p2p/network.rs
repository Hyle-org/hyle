use crate::bus::BusMessage;
use crate::mempool::MempoolNetMessage;
use crate::model::ValidatorPublicKey;
use anyhow::Context;
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_model::{ConsensusNetMessage, SignedByValidator};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{self, Display};
use strum_macros::IntoStaticStr;

#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
pub struct Hello {
    pub version: u16,
    pub validator_pubkey: ValidatorPublicKey,
    pub name: String,
    pub da_address: String,
}

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
            NetMessage::HandshakeMessage(_) => {
                write!(f, "{}", enum_variant)
            }
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
pub enum NetMessage {
    HandshakeMessage(HandshakeNetMessage),
    MempoolMessage(SignedByValidator<MempoolNetMessage>),
    ConsensusMessage(SignedByValidator<ConsensusNetMessage>),
}

#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
pub enum HandshakeNetMessage {
    Hello(Hello),
    Verack,
    Ping,
    Pong,
}

impl From<HandshakeNetMessage> for NetMessage {
    fn from(msg: HandshakeNetMessage) -> Self {
        NetMessage::HandshakeMessage(msg)
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
