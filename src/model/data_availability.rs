use bincode::{Decode, Encode};
use std::collections::HashMap;

use crate::model::{BlobsHash, BlockHeight, ContractName};
use hyle_contract_sdk::{HyleOutput, Identity, ProgramId, StateDigest, TxHash, Verifier};
use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct Contract {
    pub name: ContractName,
    pub program_id: ProgramId,
    pub state: StateDigest,
    pub verifier: Verifier,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct UnsettledBlobTransaction {
    pub identity: Identity,
    pub hash: TxHash,
    pub blobs_hash: BlobsHash,
    pub blobs: Vec<UnsettledBlobMetadata>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct UnsettledBlobMetadata {
    pub contract_name: ContractName,
    // Each time we receive a proof, we add it to this list
    pub metadata: Vec<HyleOutput>,
}

#[derive(Default, Debug, Clone, Encode, Decode)]
pub struct Timeouts {
    by_tx_hash: HashMap<TxHash, BlockHeight>,
    by_block: HashMap<BlockHeight, Vec<TxHash>>,
}

impl Timeouts {
    pub fn drop(&mut self, at: &BlockHeight) -> Vec<TxHash> {
        if let Some(vec) = self.by_block.remove(at) {
            self.by_tx_hash.retain(|_, v| v != at);
            return vec;
        }
        vec![]
    }

    pub fn list_timeouts(&self, at: &BlockHeight) -> Option<&Vec<TxHash>> {
        self.by_block.get(at)
    }
    pub fn get(&self, tx: &TxHash) -> Option<&BlockHeight> {
        self.by_tx_hash.get(tx)
    }

    /// Set timeout for a tx, overrides existing if exists
    /// TODO: try to settle following transactions when we timeout a tx
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
