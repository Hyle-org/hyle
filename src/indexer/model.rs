use crate::model::{
    BlobData, BlobReference, BlockHeight, ContractName, Identity, StateDigest, TransactionData,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Transaction<'a> {
    pub block_height: BlockHeight,
    pub tx_index: usize,
    pub tx_hash: Cow<'a, String>,
    pub data: Cow<'a, TransactionData>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TransactionOwned {
    pub block_height: BlockHeight,
    pub tx_index: usize,
    pub tx_hash: String,
    pub data: TransactionData,
}

impl<'a> From<Transaction<'a>> for TransactionOwned {
    fn from(value: Transaction) -> Self {
        Self {
            block_height: value.block_height,
            tx_index: value.tx_index,
            tx_hash: value.tx_hash.into_owned(),
            data: value.data.into_owned(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Blob<'a> {
    pub identity: Cow<'a, Identity>,
    pub contract_name: Cow<'a, ContractName>,
    pub data: Cow<'a, BlobData>,
    // refs:
    pub block_height: BlockHeight,
    pub tx_index: usize,
    pub tx_hash: Cow<'a, String>,
    pub blob_index: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Proof<'a> {
    pub blobs_references: Cow<'a, Vec<BlobReference>>,
    pub proof: Cow<'a, Vec<u8>>,
    // refs:
    pub block_height: BlockHeight,
    pub tx_index: usize,
    pub tx_hash: Cow<'a, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Contract<'a> {
    pub contract_name: Cow<'a, ContractName>,
    pub owner: Cow<'a, String>,
    pub program_id: Cow<'a, Vec<u8>>,
    pub verifier: Cow<'a, String>,
    pub state_digest: Cow<'a, StateDigest>,
}
