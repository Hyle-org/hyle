use crate::bus::BusMessage;
use crate::model::{Transaction, ValidatorPublicKey};
use crate::utils::crypto::SignedByValidator;
use crate::{consensus::ConsensusNetMessage, mempool::MempoolNetMessage};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{self, Display};
use strum_macros::IntoStaticStr;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
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
        msg: PeerNetMessage,
    },
    BroadcastMessage(PeerNetMessage),
    BroadcastMessageOnlyFor(HashSet<ValidatorPublicKey>, PeerNetMessage),
}

impl OutboundMessage {
    pub fn broadcast<T: Into<PeerNetMessage>>(msg: T) -> Self {
        OutboundMessage::BroadcastMessage(msg.into())
    }
    pub fn broadcast_only_for<T: Into<PeerNetMessage>>(
        only_for: HashSet<ValidatorPublicKey>,
        msg: T,
    ) -> Self {
        OutboundMessage::BroadcastMessageOnlyFor(only_for, msg.into())
    }
    pub fn send<T: Into<PeerNetMessage>>(validator_id: ValidatorPublicKey, msg: T) -> Self {
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

impl Display for PeerNetMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let enum_variant: &'static str = self.into();
        match self {
            PeerNetMessage::HandshakeMessage(_) => {
                write!(f, "{}", enum_variant)
            }
            PeerNetMessage::MempoolMessage(msg) => {
                _ = write!(f, "NetMessage::{} ", enum_variant);
                write!(f, "{}", msg)
            }
            PeerNetMessage::ConsensusMessage(msg) => {
                _ = write!(f, "NetMessage::{} ", enum_variant);
                write!(f, "{}", msg)
            }
        }
    }
}

impl Display for NetMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let enum_variant: &'static str = self.into();
        match self {
            NetMessage::NewTx(msg) => {
                _ = write!(f, "NetMessage::{} ", enum_variant);
                write!(f, "{:?}", msg)
            }
            NetMessage::Ping => {
                _ = write!(f, "NetMessage::{} ", enum_variant);
                write!(f, "{}", enum_variant)
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum PeerNetMessage {
    HandshakeMessage(HandshakeNetMessage),
    MempoolMessage(SignedByValidator<MempoolNetMessage>),
    ConsensusMessage(SignedByValidator<ConsensusNetMessage>),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum NetMessage {
    Ping,
    NewTx(Transaction),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum HandshakeNetMessage {
    Hello(Hello),
    Verack,
    Ping,
    Pong,
}

impl From<HandshakeNetMessage> for PeerNetMessage {
    fn from(msg: HandshakeNetMessage) -> Self {
        PeerNetMessage::HandshakeMessage(msg)
    }
}

impl From<SignedByValidator<MempoolNetMessage>> for PeerNetMessage {
    fn from(msg: SignedByValidator<MempoolNetMessage>) -> Self {
        PeerNetMessage::MempoolMessage(msg)
    }
}

impl From<SignedByValidator<ConsensusNetMessage>> for PeerNetMessage {
    fn from(msg: SignedByValidator<ConsensusNetMessage>) -> Self {
        PeerNetMessage::ConsensusMessage(msg)
    }
}

impl PeerNetMessage {
    pub fn to_binary(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard())
            .expect("Could not serialize NetMessage")
    }
}
