#![allow(dead_code, unused_variables)]
use std::collections::HashMap;

use crate::model::BlockHeight;
use crate::model::ContractName;
use crate::model::Identity;
use crate::model::TxHash;

#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub struct BlobsHash(String);

#[derive(Default, Debug, Clone)]
pub struct Timeouts {
    by_block: HashMap<BlockHeight, Vec<TxHash>>,
    by_tx_hash: HashMap<TxHash, BlockHeight>,
}

#[derive(Default, Debug, Clone)]
pub struct Contract {
    pub name: ContractName,
    pub program_id: u64,
    pub state: Vec<u8>,
}

#[derive(Default, Debug, Clone)]
pub struct UnsettledTransaction {
    pub identity: Identity,
    pub hash: TxHash,
    pub blobs_hash: BlobsHash,
    pub blobs: Vec<UnsettledBlobDetail>,
}

#[derive(Default, Debug, Clone)]
pub struct UnsettledBlobDetail {
    pub contract_name: String,
    pub verification_status: VerificationStatus,
    pub hyle_output: Option<HyleOutput>,
}

#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub enum VerificationStatus {
    #[default]
    WaitingProof,
    Success,
    InvalidProof,
    ExecutionFailed,
}

#[derive(Default, Debug, Clone)]
pub struct HyleOutput {
    pub version: u32,
    pub initial_state: Vec<u8>,
    pub next_state: Vec<u8>,
    pub identity: String,
    pub tx_hash: Vec<u8>,
    pub index: u32,
    pub payloads: Vec<u8>,
    pub success: bool,
}

impl Timeouts {
    pub fn list_timeouts(&self, at: BlockHeight) {}
    pub fn get(&self, tx: &TxHash) -> BlockHeight {
        todo!()
    }
    pub fn set(&mut self, tx: TxHash, at: BlockHeight) {}
}
