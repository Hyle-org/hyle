use crate::model::{
    BlobData, BlobReference, BlockHeight, ContractName, Identity, StateDigest, TransactionData,
};
use nocow::nocow;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[nocow(Transaction)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TransactionCow<'a> {
    pub tx_hash: Cow<'a, String>,
    pub data: Cow<'a, TransactionData>,
    // refs:
    pub block_height: BlockHeight,
    pub tx_index: usize,
}

#[nocow(Blob)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BlobCow<'a> {
    pub identity: Cow<'a, Identity>,
    pub contract_name: Cow<'a, ContractName>,
    pub data: Cow<'a, BlobData>,
    // refs:
    pub block_height: BlockHeight,
    pub tx_index: usize,
    pub tx_hash: Cow<'a, String>,
    pub blob_index: usize,
}

#[nocow(Proof)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProofCow<'a> {
    pub blobs_references: Cow<'a, Vec<BlobReference>>,
    pub proof: Cow<'a, Vec<u8>>,
    // refs:
    pub block_height: BlockHeight,
    pub tx_index: usize,
    pub tx_hash: Cow<'a, String>,
}

#[nocow(Contract)]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContractCow<'a> {
    pub contract_name: Cow<'a, ContractName>,
    pub owner: Cow<'a, String>,
    pub program_id: Cow<'a, Vec<u8>>,
    pub verifier: Cow<'a, String>,
    pub state_digest: Cow<'a, StateDigest>,
    // refs:
    pub block_height: BlockHeight,
    pub tx_index: usize,
    pub tx_hash: Cow<'a, String>,
}
