use borsh::{BorshDeserialize, BorshSerialize};
use hyle_model::TxHash;
use tracing::warn;

use crate::model::ContractName;
use crate::model::UnsettledBlobTransaction;
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};

// struct used to guarantee coherence between the 2 fields
#[derive(Default, Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct OrderedTxMap {
    map: HashMap<TxHash, UnsettledBlobTransaction>,
    tx_order: HashMap<ContractName, VecDeque<TxHash>>,
}

impl OrderedTxMap {
    #[allow(dead_code)]
    pub fn get(&self, hash: &TxHash) -> Option<&UnsettledBlobTransaction> {
        self.map.get(hash)
    }

    /// Returns true if the tx is the next to settle for all the contracts it contains
    pub fn is_next_to_settle(&self, tx_hash: &TxHash) -> bool {
        if let Some(unsettled_blob_tx) = self.map.get(tx_hash) {
            unsettled_blob_tx.blobs.iter().all(|blob_metadata| {
                if self.get_next_unsettled_tx(&blob_metadata.blob.contract_name) != Some(tx_hash) {
                    return false;
                }
                // The tx is the next to settle for all the contracts it contains
                true
            })
        } else {
            false
        }
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

    /// Returns true if the tx is the next unsettled tx for all the contracts it contains
    /// NB: not if the TX was already added to the map, which feels like it generally shouldn't happen?
    pub fn add(&mut self, tx: UnsettledBlobTransaction) -> bool {
        if self.map.contains_key(&tx.hash) {
            warn!("Trying to add a tx {} that is already in the map", tx.hash);
            return false;
        }
        let mut is_next = true;
        let contract_names: HashSet<&ContractName> =
            HashSet::from_iter(tx.blobs.iter().map(|b| &b.blob.contract_name));
        for contract in contract_names {
            is_next = match self.tx_order.get_mut(contract) {
                Some(vec) => {
                    vec.push_back(tx.hash.clone());
                    vec.len() == 1
                }
                None => {
                    self.tx_order.insert(contract.clone(), {
                        let mut vec = VecDeque::new();
                        vec.push_back(tx.hash.clone());
                        vec
                    });
                    true
                }
            } && is_next;
        }

        self.map.insert(tx.hash.clone(), tx);
        is_next
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
    use std::sync::Arc;

    use crate::model::{BlobsHash, UnsettledBlobMetadata};
    use hyle_contract_sdk::{Blob, BlobData, Identity};
    use hyle_model::{DataProposalHash, TxContext, TxHash};

    use super::*;

    fn new_tx(hash: &str, contract: &str) -> UnsettledBlobTransaction {
        UnsettledBlobTransaction {
            identity: Identity::new("toto"),
            hash: TxHash::new(hash),
            parent_dp_hash: DataProposalHash::default(),
            blobs_hash: BlobsHash::new("blobs_hash"),
            blobs: vec![UnsettledBlobMetadata {
                blob: Blob {
                    contract_name: ContractName(contract.to_string()),
                    data: BlobData::default(),
                },
                possible_proofs: vec![],
            }],
            tx_context: Arc::new(TxContext::default()),
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
    fn add_double_contract_name() {
        let mut map = OrderedTxMap::default();

        let mut tx = new_tx("tx1", "c1");
        tx.blobs.push(tx.blobs[0].clone());
        tx.blobs.push(tx.blobs[0].clone());
        tx.blobs[1].blob.contract_name = ContractName::new("c2");

        let hash = tx.hash.clone();

        assert!(map.add(tx));
        assert_eq!(
            map.tx_order.get(&"c1".into()),
            Some(&VecDeque::from_iter(vec![hash.clone()]))
        );
        assert_eq!(
            map.tx_order.get(&"c2".into()),
            Some(&VecDeque::from_iter(vec![hash]))
        );
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
