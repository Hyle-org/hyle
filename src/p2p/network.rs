use std::fmt;

use crate::validator_registry::{ValidatorId, ValidatorPublicKey, ValidatorRegistryNetMessage};
use crate::{consensus::ConsensusNetMessage, mempool::MempoolNetMessage};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct Version {
    pub id: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutboundMessage {
    SendMessage {
        validator_id: ValidatorId,
        msg: NetMessage,
    },
    BroadcastMessage(NetMessage),
}

impl OutboundMessage {
    pub fn broadcast<T: Into<NetMessage>>(msg: T) -> Self {
        OutboundMessage::BroadcastMessage(msg.into())
    }
    pub fn send<T: Into<NetMessage>>(validator_id: ValidatorId, msg: T) -> Self {
        OutboundMessage::SendMessage {
            validator_id,
            msg: msg.into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Encode, Decode, Default)]
pub struct Signature(pub Vec<u8>);

impl std::fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Signature")
            .field(&hex::encode(&self.0))
            .finish()
    }
}
pub type SignedWithId<T> = Signed<T, ValidatorId>;
pub type SignedWithKey<T> = Signed<T, ValidatorPublicKey>;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct Signed<T: Encode, V> {
    pub msg: T,
    pub signature: Signature,
    pub validators: Vec<V>,
}

impl<T: Encode> Signed<T, ValidatorId> {
    pub fn with_pub_keys(&self, pub_key: Vec<ValidatorPublicKey>) -> Signed<T, ValidatorPublicKey>
    where
        T: Encode + Clone,
    {
        SignedWithKey {
            msg: self.msg.clone(),
            signature: self.signature.clone(),
            validators: pub_key,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NetMessage {
    HandshakeMessage(HandshakeNetMessage),
    MempoolMessage(SignedWithId<MempoolNetMessage>),
    ConsensusMessage(SignedWithId<ConsensusNetMessage>),
    ValidatorRegistryMessage(ValidatorRegistryNetMessage),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum HandshakeNetMessage {
    Version(Version),
    Verack,
    Ping,
    Pong,
}

impl From<HandshakeNetMessage> for NetMessage {
    fn from(msg: HandshakeNetMessage) -> Self {
        NetMessage::HandshakeMessage(msg)
    }
}

impl From<ValidatorRegistryNetMessage> for NetMessage {
    fn from(msg: ValidatorRegistryNetMessage) -> Self {
        NetMessage::ValidatorRegistryMessage(msg)
    }
}

impl From<SignedWithId<MempoolNetMessage>> for NetMessage {
    fn from(msg: SignedWithId<MempoolNetMessage>) -> Self {
        NetMessage::MempoolMessage(msg)
    }
}

impl From<SignedWithId<ConsensusNetMessage>> for NetMessage {
    fn from(msg: SignedWithId<ConsensusNetMessage>) -> Self {
        NetMessage::ConsensusMessage(msg)
    }
}

impl NetMessage {
    pub fn to_binary(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard())
            .expect("Could not serialize NetMessage")
    }
}
