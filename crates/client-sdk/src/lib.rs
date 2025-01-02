use base64::prelude::*;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

pub mod transaction_builder;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Encode, Decode)]
#[serde(untagged)]
pub enum ProofData {
    Base64(String),
    Bytes(Vec<u8>),
}
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode)]
pub struct ProofDataHash(pub String);

impl Default for ProofData {
    fn default() -> Self {
        ProofData::Bytes(Vec::new())
    }
}

impl ProofData {
    pub fn to_bytes(&self) -> Result<Vec<u8>, base64::DecodeError> {
        match self {
            ProofData::Base64(s) => BASE64_STANDARD.decode(s),
            ProofData::Bytes(b) => Ok(b.clone()),
        }
    }
}
