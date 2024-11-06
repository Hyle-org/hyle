#![no_std]

extern crate alloc;

use core::fmt::Display;

use alloc::string::String;
use alloc::vec::Vec;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

pub mod erc20;
pub mod guest;
pub mod identity_provider;

pub trait HyleContract {
    fn caller(&self) -> Identity;
}

pub trait Digestable {
    fn as_digest(&self) -> StateDigest;
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ContractInput<State>
where
    State: Digestable,
{
    pub initial_state: State,
    pub identity: Identity,
    pub tx_hash: String,
    pub private_blob: BlobData,
    pub blobs: Vec<BlobData>,
    pub index: usize,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct StateDigest(pub Vec<u8>);

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct Identity(pub String);

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct TxHash(pub String);

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct BlobIndex(pub u32);

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct BlobData(pub Vec<u8>);

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct HyleOutput {
    pub version: u32,
    pub initial_state: StateDigest,
    pub next_state: StateDigest,
    pub identity: Identity,
    pub tx_hash: TxHash,
    pub index: BlobIndex,
    pub blobs: Vec<u8>,
    pub success: bool,
    pub program_outputs: Vec<u8>,
}

impl From<String> for Identity {
    fn from(s: String) -> Self {
        Identity(s)
    }
}
impl From<&str> for Identity {
    fn from(s: &str) -> Self {
        Identity(s.into())
    }
}
impl From<String> for TxHash {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for TxHash {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}

impl TxHash {
    pub fn new(s: &str) -> TxHash {
        TxHash(s.into())
    }
}
impl Display for TxHash {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl Display for BlobIndex {
    fn fmt(&self, f: &mut alloc::fmt::Formatter<'_>) -> alloc::fmt::Result {
        write!(f, "{}", &self.0)
    }
}
impl From<u32> for BlobIndex {
    fn from(i: u32) -> Self {
        BlobIndex(i)
    }
}
