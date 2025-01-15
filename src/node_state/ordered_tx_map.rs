use bincode::{Decode, Encode};

use crate::model::ContractName;
use crate::model::UnsettledBlobTransaction;
use hyle_contract_sdk::TxHash;
use std::collections::{HashMap, VecDeque};

// struct used to guarantee coherence between the 2 fields
#[derive(Default, Debug, Clone, Encode, Decode)]
pub struct OrderedTxMap {
    map: HashMap<TxHash, UnsettledBlobTransaction>,
    tx_order: HashMap<ContractName, VecDeque<TxHash>>,
}

impl OrderedTxMap {
    #[allow(dead_code)]
    pub fn get(&self, hash: &TxHash) -> Option<&UnsettledBlobTransaction> {
        self.map.get(hash)
    }

    pub fn get_next_unsettled_tx(&self, contract: &ContractName) -> Option<&TxHash> {
        self.tx_order.get(contract).and_then(|v| v.front())
    }

    pub fn get_for_settlement(
        &mut self,
        hash: &TxHash,
    ) -> Option<(&mut UnsettledBlobTransaction, bool)> {
        let tx = self.map.get_mut(hash);
        match tx {
            Some(tx) => {
                let is_next_unsettled_tx = tx.blobs.iter().all(|blob_metadata| {
                    if let Some(order) = self.tx_order.get(&blob_metadata.blob.contract_name) {
                        if let Some(first) = order.front() {
                            return first == &tx.hash;
                        }
                    }
                    false
                });
                Some((tx, is_next_unsettled_tx))
            }
            None => None,
        }
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn add(&mut self, tx: UnsettledBlobTransaction) {
        if self.map.contains_key(&tx.hash) {
            return;
        }
        for blob_metadata in &tx.blobs {
            match self.tx_order.get_mut(&blob_metadata.blob.contract_name) {
                Some(vec) => {
                    vec.push_back(tx.hash.clone());
                }
                None => {
                    self.tx_order
                        .insert(blob_metadata.blob.contract_name.clone(), {
                            let mut vec = VecDeque::new();
                            vec.push_back(tx.hash.clone());
                            vec
                        });
                }
            }
        }

        self.map.insert(tx.hash.clone(), tx);
    }

    pub fn remove(&mut self, hash: &TxHash) -> Option<UnsettledBlobTransaction> {
        if let Some(tx) = self.map.get(hash) {
            for blob_metadata in &tx.blobs {
                if let Some(c) = self.tx_order.get_mut(&blob_metadata.blob.contract_name) {
                    if let Some(t) = c.front() {
                        if t.eq(hash) {
                            c.pop_front();
                        } else {
                            // Panic - this indicates a logic error in the code
                            panic!("Trying to remove a tx that is not the first in the queue");
                        }
                    }
                }
            }
            self.map.remove(hash)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use crate::model::{BlobsHash, UnsettledBlobMetadata};
    use hyle_contract_sdk::{Blob, BlobData, Identity};

    use super::*;

    fn new_tx(hash: &str, contract: &str) -> UnsettledBlobTransaction {
        UnsettledBlobTransaction {
            identity: Identity::new("toto"),
            hash: TxHash::new(hash),
            blobs_hash: BlobsHash::new("blobs_hash"),
            blobs: vec![UnsettledBlobMetadata {
                blob: Blob {
                    contract_name: ContractName(contract.to_string()),
                    data: BlobData::default(),
                },
                possible_proofs: vec![],
            }],
        }
    }

    fn is_next_unsettled_tx(map: &mut OrderedTxMap, hash: &TxHash) -> bool {
        let Some((_, is_next)) = map.get_for_settlement(hash) else {
            return false;
        };
        is_next
    }

    #[test]
    fn can_add_tx() {
        let mut map = OrderedTxMap::default();
        let tx = new_tx("tx1", "contract1");

        map.add(tx);
        assert_eq!(map.map.len(), 1);
        assert_eq!(map.tx_order.len(), 1);
    }

    #[test]
    fn can_get_tx() {
        let mut map = OrderedTxMap::default();
        let tx1 = TxHash::new("tx1");
        let tx2 = TxHash::new("tx2");
        let tx3 = TxHash::new("tx3");

        map.add(new_tx("tx1", "c1"));
        map.add(new_tx("tx2", "c1"));
        map.add(new_tx("tx3", "c2"));

        assert_eq!(tx1, map.get(&tx1).unwrap().hash);
        assert_eq!(tx2, map.get(&tx2).unwrap().hash);
        assert_eq!(tx3, map.get(&tx3).unwrap().hash);

        assert_eq!(map.map.len(), 3);
        assert_eq!(map.tx_order.len(), 2);
    }

    #[test]
    fn double_add_ignored() {
        let mut map = OrderedTxMap::default();
        let tx1 = TxHash::new("tx1");

        map.add(new_tx("tx1", "c1"));
        map.add(new_tx("tx1", "c1"));

        assert_eq!(tx1, map.get(&tx1).unwrap().hash);

        assert_eq!(map.map.len(), 1);
        assert_eq!(map.tx_order.len(), 1);
        assert_eq!(map.tx_order[&ContractName::new("c1")].len(), 1);
    }

    #[test]
    fn check_next_unsettled_tx() {
        let mut map = OrderedTxMap::default();
        let tx1 = TxHash::new("tx1");
        let tx2 = TxHash::new("tx2");
        let tx3 = TxHash::new("tx3");
        let tx4 = TxHash::new("tx4");

        map.add(new_tx("tx1", "c1"));
        map.add(new_tx("tx2", "c1"));
        map.add(new_tx("tx3", "c2"));

        assert!(is_next_unsettled_tx(&mut map, &tx1));
        assert!(!is_next_unsettled_tx(&mut map, &tx2));
        assert!(is_next_unsettled_tx(&mut map, &tx3));
        // tx doesn't even exit
        assert!(!is_next_unsettled_tx(&mut map, &tx4));
    }

    #[test]
    fn remove_tx() {
        let mut map = OrderedTxMap::default();
        let tx1 = TxHash::new("tx1");
        let tx2 = TxHash::new("tx2");
        let tx3 = TxHash::new("tx3");
        let c1 = ContractName::new("c1");

        map.add(new_tx("tx1", "c1"));
        map.add(new_tx("tx2", "c1"));
        map.add(new_tx("tx3", "c2"));
        map.remove(&tx1);

        assert!(!is_next_unsettled_tx(&mut map, &tx1));
        assert!(is_next_unsettled_tx(&mut map, &tx2));
        assert!(is_next_unsettled_tx(&mut map, &tx3));

        assert_eq!(map.map.len(), 2);
        assert_eq!(map.tx_order.len(), 2);
        assert_eq!(map.tx_order[&c1].len(), 1);

        map.remove(&tx2);
        assert_eq!(map.map.len(), 1);
        assert_eq!(map.tx_order.len(), 2);
        assert_eq!(map.tx_order[&c1].len(), 0);
    }
}
