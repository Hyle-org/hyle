#![allow(dead_code, unused_variables)]
use std::collections::HashMap;

use crate::model::BlobsHash;
use crate::model::BlockHeight;
use crate::model::ContractName;
use crate::model::Identity;
use crate::model::TxHash;

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
    pub contract_name: ContractName,
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

#[derive(Default, Debug, Clone)]
pub struct Timeouts {
    by_tx_hash: HashMap<TxHash, BlockHeight>,
    by_block: HashMap<BlockHeight, Vec<TxHash>>,
}

impl Timeouts {
    pub fn drop(&mut self, at: &BlockHeight) {
        if let Some(vec) = self.by_block.remove(at) {
            self.by_tx_hash.retain(|k, v| v != at);
        }
    }
    pub fn list_timeouts(&self, at: &BlockHeight) -> Option<&Vec<TxHash>> {
        self.by_block.get(at)
    }
    pub fn get(&self, tx: &TxHash) -> Option<&BlockHeight> {
        self.by_tx_hash.get(tx)
    }

    /// Set timeout for a tx, overrides existing if exists
    pub fn set(&mut self, tx: TxHash, at: BlockHeight) {
        if let Some(existing) = self.get(&tx) {
            let existing2 = *existing; // copy
            if let Some(vec) = self.by_block.get_mut(&existing2) {
                vec.retain(|t| !t.eq(&tx));
            }
        }
        self.by_tx_hash.insert(tx.clone(), at);

        match self.by_block.get_mut(&at) {
            Some(vec) => {
                vec.push(tx);
            }
            None => {
                self.by_block.insert(at, vec![tx]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout() {
        let mut t = Timeouts::default();
        let b1 = BlockHeight(0);
        let b2 = BlockHeight(1);
        let tx1 = TxHash::new("tx1");

        t.set(tx1.clone(), b1);

        assert_eq!(t.list_timeouts(&b1).unwrap().len(), 1);
        assert_eq!(t.list_timeouts(&b2), None);
        assert_eq!(t.get(&tx1), Some(&b1));

        t.set(tx1.clone(), b2);
        assert_eq!(t.get(&tx1), Some(&b2));

        t.drop(&b1);
        assert_eq!(t.get(&tx1), Some(&b2));
        assert_eq!(t.list_timeouts(&b1), None);
        assert_eq!(t.list_timeouts(&b2).unwrap().len(), 1);

        t.drop(&b2);
        assert_eq!(t.get(&tx1), None);
        assert_eq!(t.list_timeouts(&b2), None);
    }
}
