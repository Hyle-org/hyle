use crate::bus::BusMessage;
use crate::model::ValidatorPublicKey;
use crate::{consensus::ConsensusNetMessage, mempool::MempoolNetMessage};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub struct Hello {
    pub version: u16,
    pub validator_pubkey: ValidatorPublicKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutboundMessage {
    SendMessage {
        validator_id: ValidatorPublicKey,
        msg: NetMessage,
    },
    BroadcastMessage(NetMessage),
}

impl OutboundMessage {
    pub fn broadcast<T: Into<NetMessage>>(msg: T) -> Self {
        OutboundMessage::BroadcastMessage(msg.into())
    }
    pub fn send<T: Into<NetMessage>>(validator_id: ValidatorPublicKey, msg: T) -> Self {
        OutboundMessage::SendMessage {
            validator_id,
            msg: msg.into(),
        }
    }
}
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

impl<T> BusMessage for SignedWithKey<T> where T: Encode + BusMessage {}

pub type SignedWithKey<T> = Signed<T, ValidatorPublicKey>;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct Signed<T: Encode, V> {
    pub msg: T,
    pub signature: Signature,
    pub validators: Vec<V>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum NetMessage {
    HandshakeMessage(HandshakeNetMessage),
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
