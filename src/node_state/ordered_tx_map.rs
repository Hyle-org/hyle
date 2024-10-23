use bincode::{Decode, Encode};
use hyle_contract_sdk::TxHash;

use super::model::UnsettledTransaction;
use crate::model::{BlobsHash, ContractName};
use std::collections::HashMap;

// struct used to guarantee coherence between the 2 fields
#[derive(Default, Debug, Clone, Encode, Decode)]
pub struct OrderedTxMap {
    map: HashMap<BlobsHash, UnsettledTransaction>,
    // To store mapping between tx_hash and blobs.hash()
    blobs_hash_for_fees: HashMap<TxHash, BlobsHash>,
    blobs_hash_for_blobs: HashMap<TxHash, BlobsHash>,
    tx_order: HashMap<ContractName, Vec<BlobsHash>>,
}

impl OrderedTxMap {
    pub fn get(&self, hash: &BlobsHash) -> Option<&UnsettledTransaction> {
        self.map.get(hash)
    }

    pub fn get_mut(&mut self, hash: &BlobsHash) -> Option<&mut UnsettledTransaction> {
        self.map.get_mut(hash)
    }

    pub fn get_for(&self, hash: &TxHash, fees: bool) -> Option<&UnsettledTransaction> {
        if fees {
            self.get_for_fees(hash)
        } else {
            self.get_for_blobs(hash)
        }
    }

    pub fn get_for_blobs(&self, hash: &TxHash) -> Option<&UnsettledTransaction> {
        self.blobs_hash_for_blobs
            .get(hash)
            .and_then(|h| self.map.get(h))
    }

    pub fn get_for_fees(&self, hash: &TxHash) -> Option<&UnsettledTransaction> {
        self.blobs_hash_for_fees
            .get(hash)
            .and_then(|h| self.map.get(h))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn add(&mut self, tx: UnsettledTransaction, fees: bool) {
        if self.map.contains_key(&tx.blobs_hash) {
            return;
        }
        for blob in &tx.blobs {
            match self.tx_order.get_mut(&blob.contract_name) {
                Some(vec) => {
                    vec.push(tx.blobs_hash.clone());
                }
                None => {
                    self.tx_order
                        .insert(blob.contract_name.clone(), vec![tx.blobs_hash.clone()]);
                }
            }
        }
        if fees {
            self.blobs_hash_for_fees
                .insert(tx.tx_hash.clone(), tx.blobs_hash.clone());
        } else {
            self.blobs_hash_for_blobs
                .insert(tx.tx_hash.clone(), tx.blobs_hash.clone());
        }

        self.map.insert(tx.blobs_hash.clone(), tx);
    }

    pub fn remove(&mut self, hash: &BlobsHash) {
        if let Some(tx) = self.map.get(hash) {
            for blob in &tx.blobs {
                if let Some(c) = self.tx_order.get_mut(&blob.contract_name) {
                    c.retain(|h| !h.eq(hash));
                }
            }
            self.blobs_hash_for_blobs.remove(&tx.tx_hash);
            self.blobs_hash_for_fees.remove(&tx.tx_hash);
        }
        self.map.remove(hash);
    }

    pub fn remove_for_blob(&mut self, hash: &TxHash) {
        let blob_hash = self.blobs_hash_for_blobs.remove(hash);
        if let Some(tx) = blob_hash.clone().and_then(|h| self.map.get(&h)) {
            for blob in &tx.blobs {
                if let Some(c) = self.tx_order.get_mut(&blob.contract_name) {
                    c.retain(|h| !h.eq(&tx.blobs_hash));
                }
            }
        }
        blob_hash.and_then(|h| self.map.remove(&h));
    }

    pub fn is_next_unsettled_tx(&self, tx: &BlobsHash) -> bool {
        match self.get(tx) {
            Some(unsettled_tx) => {
                for blob in &unsettled_tx.blobs {
                    if let Some(order) = self.tx_order.get(&blob.contract_name) {
                        if let Some(first) = order.first() {
                            if first != tx {
                                return false;
                            }
                        }
                    }
                }
            }
            None => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::{model::BlobsHash, node_state::model::UnsettledBlobMetadata};
    use hyle_contract_sdk::Identity;

    use super::*;

    fn new_tx(hash: &str, contract: &str, blobs_hash: &str) -> UnsettledTransaction {
        UnsettledTransaction {
            identity: Identity("toto".to_string()),
            tx_hash: TxHash::new(hash),
            blobs_hash: BlobsHash::new(blobs_hash),
            blobs: vec![UnsettledBlobMetadata {
                contract_name: ContractName(contract.to_string()),
                metadata: vec![],
            }],
        }
    }

    #[test]
    fn can_add_tx() {
        let mut map = OrderedTxMap::default();
        let tx = new_tx("tx1", "contract1", "blobs_hash1");

        map.add(tx, false);
        assert_eq!(map.map.len(), 1);
        assert_eq!(map.tx_order.len(), 1);
    }

    #[test]
    fn can_get_tx() {
        let mut map = OrderedTxMap::default();
        let blobs_hash1 = BlobsHash::new("blobs_hash1");
        let blobs_hash2 = BlobsHash::new("blobs_hash2");
        let blobs_hash3 = BlobsHash::new("blobs_hash3");

        let tx1 = new_tx("tx1", "c1", "blobs_hash1");
        let tx2 = new_tx("tx2", "c1", "blobs_hash2");
        let tx3 = new_tx("tx3", "c2", "blobs_hash3");

        map.add(tx1.clone(), false);
        map.add(tx2.clone(), false);
        map.add(tx3.clone(), false);

        assert_eq!(blobs_hash1, map.get(&blobs_hash1).unwrap().blobs_hash);
        assert_eq!(blobs_hash2, map.get(&blobs_hash2).unwrap().blobs_hash);
        assert_eq!(blobs_hash3, map.get(&blobs_hash3).unwrap().blobs_hash);

        assert_eq!(map.map.len(), 3);
        assert_eq!(map.tx_order.len(), 2);
    }

    #[test]
    fn double_add_ignored() {
        let mut map = OrderedTxMap::default();
        let blobs_hash1 = BlobsHash::new("blobs_hash1");

        let tx1 = new_tx("tx1", "c1", "blobs_hash1");

        map.add(tx1.clone(), false);
        map.add(tx1.clone(), false); // Add the same transaction again

        assert_eq!(blobs_hash1, map.get(&blobs_hash1).unwrap().blobs_hash);

        assert_eq!(map.map.len(), 1);
        assert_eq!(map.tx_order.len(), 1);
        assert_eq!(map.tx_order[&ContractName("c1".to_string())].len(), 1);
    }

    #[test]
    fn check_next_unsettled_tx() {
        let mut map = OrderedTxMap::default();
        let blobs_hash1 = BlobsHash::new("blobs_hash1");
        let blobs_hash2 = BlobsHash::new("blobs_hash2");
        let blobs_hash3 = BlobsHash::new("blobs_hash3");

        let tx1 = new_tx("tx1", "c1", "blobs_hash1");
        let tx2 = new_tx("tx2", "c1", "blobs_hash2");
        let tx3 = new_tx("tx3", "c2", "blobs_hash3");

        map.add(tx1.clone(), false);
        map.add(tx2.clone(), false);
        map.add(tx3.clone(), false);

        assert!(map.is_next_unsettled_tx(&blobs_hash1));
        assert!(!map.is_next_unsettled_tx(&blobs_hash2));
        assert!(map.is_next_unsettled_tx(&blobs_hash3));

        let blobs_hash4 = BlobsHash::new("blobs_hash4");
        assert!(!map.is_next_unsettled_tx(&blobs_hash4)); // Non-existent transaction
    }

    #[test]
    fn remove_tx() {
        let mut map = OrderedTxMap::default();
        let blobs_hash1 = BlobsHash::new("blobs_hash1");
        let blobs_hash2 = BlobsHash::new("blobs_hash2");
        let blobs_hash3 = BlobsHash::new("blobs_hash3");
        let c1 = ContractName("c1".to_string());

        let tx1 = new_tx("tx1", "c1", "blobs_hash1");
        let tx2 = new_tx("tx2", "c1", "blobs_hash2");
        let tx3 = new_tx("tx3", "c2", "blobs_hash3");

        map.add(tx1.clone(), false);
        map.add(tx2.clone(), false);
        map.add(tx3.clone(), false);

        map.remove(&blobs_hash1);

        assert!(!map.is_next_unsettled_tx(&blobs_hash1));
        assert!(map.is_next_unsettled_tx(&blobs_hash2));
        assert!(map.is_next_unsettled_tx(&blobs_hash3));

        assert_eq!(map.map.len(), 2);
        assert_eq!(map.tx_order.len(), 2);
        assert_eq!(map.tx_order[&c1].len(), 1);

        map.remove(&blobs_hash2);
        assert_eq!(map.map.len(), 1);
        assert_eq!(map.tx_order.len(), 2);
        assert_eq!(map.tx_order[&c1].len(), 0);
    }
}
