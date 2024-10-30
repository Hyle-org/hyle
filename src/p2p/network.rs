use crate::bus::BusMessage;
use crate::consensus::utils::HASH_DISPLAY_SIZE;
use crate::data_availability::DataNetMessage;
use crate::model::ValidatorPublicKey;
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
    },
}

impl BusMessage for PeerEvent {}
impl BusMessage for OutboundMessage {}

#[derive(Serialize, Deserialize, Clone, Encode, Decode, Default, PartialEq, Eq, Hash)]
pub struct Signature(pub Vec<u8>);

impl std::fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Signature")
            .field(&hex::encode(&self.0))
            .finish()
    }
}

impl Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            &self
                .0
                .get(..HASH_DISPLAY_SIZE)
                .map(hex::encode)
                .unwrap_or_default()
        )
    }
}

impl Display for NetMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let enum_variant: &'static str = self.into();
        match self {
            NetMessage::HandshakeMessage(_) => {
                write!(f, "{}", enum_variant)
            }
            NetMessage::DataMessage(_) => {
                write!(f, "{}", enum_variant)
            }
            NetMessage::MempoolMessage(signed_msg) => {
                _ = write!(f, "NetMessage::{} ", enum_variant);
                write!(f, "{}", signed_msg)
            }
            NetMessage::ConsensusMessage(signed_msg) => {
                _ = write!(f, "NetMessage::{} ", enum_variant);
                write!(f, "{}", signed_msg)
            }
        }
    }
}

impl<T: Display + bincode::Encode> Display for Signed<T, ValidatorPublicKey> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        _ = write!(f, "{}", self.msg);
        _ = write!(f, "\nSigned with {} and validators ", self.signature);
        for v in self.validators.iter() {
            _ = write!(f, "{},", v);
        }
        write!(f, "")
    }
}

impl<T> BusMessage for SignedWithKey<T> where T: Encode + BusMessage {}

pub type SignedWithKey<T> = Signed<T, ValidatorPublicKey>;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct Signed<T: Encode, V> {
    pub msg: T,
    pub signature: Signature,
    pub validators: Vec<V>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum NetMessage {
    HandshakeMessage(HandshakeNetMessage),
    DataMessage(DataNetMessage),
    MempoolMessage(SignedWithKey<MempoolNetMessage>),
    ConsensusMessage(SignedWithKey<ConsensusNetMessage>),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
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

impl From<SignedWithKey<MempoolNetMessage>> for NetMessage {
    fn from(msg: SignedWithKey<MempoolNetMessage>) -> Self {
        NetMessage::MempoolMessage(msg)
    }
}

impl From<SignedWithKey<ConsensusNetMessage>> for NetMessage {
    fn from(msg: SignedWithKey<ConsensusNetMessage>) -> Self {
        NetMessage::ConsensusMessage(msg)
    }
}

impl NetMessage {
    pub fn to_binary(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard())
            .expect("Could not serialize NetMessage")
    }
}
