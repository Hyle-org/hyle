use crate::bus::BusMessage;
use crate::validator_registry::{ValidatorId, ValidatorPublicKey, ValidatorRegistryNetMessage};
use crate::{consensus::ConsensusNetMessage, mempool::MempoolNetMessage};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fmt;
use tokio_util::bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

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

impl<T> BusMessage for SignedWithId<T> where T: Encode + BusMessage {}
impl<T> BusMessage for SignedWithKey<T> where T: Encode + BusMessage {}

pub type SignedWithId<T> = Signed<T, ValidatorId>;
pub type SignedWithKey<T> = Signed<T, ValidatorPublicKey>;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
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

pub struct NetMessageCodec;

impl Decoder for NetMessageCodec {
    type Item = NetMessage;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // We expect at least 4 bytes for the message size
        if src.len() < 4 {
            return Ok(None);
        }

        let msg_size = u32::from_be_bytes(src[..4].try_into()?);

        if src.len() < (msg_size + 4) as usize {
            // Not enough bytes yet, wait for more data
            return Ok(None);
        }

        // Split off the first 4 bytes (size) and then the message itself
        let _ = src.split_to(4);
        let data = src.split_to(msg_size as usize);

        let (net_msg, _) = bincode::decode_from_slice(&data, bincode::config::standard())
            .map_err(|_| anyhow::anyhow!("Could not decode NetMessage"))?;
        Ok(Some(net_msg))
    }
}

impl Encoder<NetMessage> for NetMessageCodec {
    type Error = anyhow::Error;

    fn encode(&mut self, item: NetMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let binary = item.to_binary();
        let size = (binary.len() as u32).to_be_bytes();
        dst.put(&size[..]);
        dst.put(&binary[..]);
        Ok(())
    }
}
