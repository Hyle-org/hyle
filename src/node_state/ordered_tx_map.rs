use super::model::UnsettledTransaction;
use crate::model::{ContractName, TxHash};
use std::collections::HashMap;

// struct used to guarantee coherence between the 2 fields
#[derive(Default, Debug, Clone)]
pub struct OrderedTxMap {
    map: HashMap<TxHash, UnsettledTransaction>,
    tx_order: HashMap<ContractName, Vec<TxHash>>,
}

impl OrderedTxMap {
    pub fn get(&self, hash: &TxHash) -> Option<&UnsettledTransaction> {
        self.map.get(hash)
    }

    pub fn get_mut(&mut self, hash: &TxHash) -> Option<&mut UnsettledTransaction> {
        self.map.get_mut(hash)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn add(&mut self, tx: UnsettledTransaction) {
        if self.map.contains_key(&tx.hash) {
            return;
        }
        for blob in &tx.blobs {
            match self.tx_order.get_mut(&blob.contract_name) {
                Some(vec) => {
                    vec.push(tx.hash.clone());
                }
                None => {
                    self.tx_order
                        .insert(blob.contract_name.clone(), vec![tx.hash.clone()]);
                }
            }
        }

        self.map.insert(tx.hash.clone(), tx);
    }

    pub fn remove(&mut self, hash: &TxHash) {
        if let Some(tx) = self.map.get(hash) {
            for blob in &tx.blobs {
                if let Some(c) = self.tx_order.get_mut(&blob.contract_name) {
                    c.retain(|h| !h.eq(hash));
                }
            }
        }
        self.map.remove(hash);
    }

    pub fn is_next_unsettled_tx(&self, tx: &TxHash, contract: &ContractName) -> bool {
        if let Some(order) = self.tx_order.get(contract) {
            if let Some(first) = order.first() {
                return first.eq(tx);
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        model::{BlobsHash, Identity},
        node_state::model::UnsettledBlobMetadata,
    };

    use super::*;

    fn new_tx(hash: &str, contract: &str) -> UnsettledTransaction {
        UnsettledTransaction {
            identity: Identity("toto".to_string()),
            hash: TxHash::new(hash),
            blobs_hash: BlobsHash::new("blobs_hash"),
            blobs: vec![UnsettledBlobMetadata {
                contract_name: ContractName(contract.to_string()),
                metadata: vec![],
            }],
        }
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
        assert_eq!(tx3, map.get_mut(&tx3).unwrap().hash);

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
        assert_eq!(map.tx_order[&ContractName("c1".to_string())].len(), 1);
    }

    #[test]
    fn check_next_unsettled_tx() {
        let mut map = OrderedTxMap::default();
        let tx1 = TxHash::new("tx1");
        let tx2 = TxHash::new("tx2");

        map.add(new_tx("tx1", "c1"));
        map.add(new_tx("tx2", "c1"));
        map.add(new_tx("tx3", "c2"));

        assert!(map.is_next_unsettled_tx(&tx1, &ContractName("c1".to_string())));
        assert!(!map.is_next_unsettled_tx(&tx2, &ContractName("c1".to_string())));
        // wrong contract
        assert!(!map.is_next_unsettled_tx(&tx2, &ContractName("c2".to_string())));
        // contract not even exits
        assert!(!map.is_next_unsettled_tx(&tx2, &ContractName("c3".to_string())));
    }

    #[test]
    fn remove_tx() {
        let mut map = OrderedTxMap::default();
        let tx1 = TxHash::new("tx1");
        let tx2 = TxHash::new("tx2");
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());
        let c3 = ContractName("c3".to_string());

        map.add(new_tx("tx1", "c1"));
        map.add(new_tx("tx2", "c1"));
        map.add(new_tx("tx3", "c2"));
        map.remove(&tx1);

        assert!(!map.is_next_unsettled_tx(&tx1, &c1));
        assert!(map.is_next_unsettled_tx(&tx2, &c1));
        // wrong contract
        assert!(!map.is_next_unsettled_tx(&tx2, &c2));
        // contract not even exits
        assert!(!map.is_next_unsettled_tx(&tx2, &c3));

        assert_eq!(map.map.len(), 2);
        assert_eq!(map.tx_order.len(), 2);
        assert_eq!(map.tx_order[&c1].len(), 1);

        map.remove(&tx2);
        assert_eq!(map.map.len(), 1);
        assert_eq!(map.tx_order.len(), 2);
        assert_eq!(map.tx_order[&c1].len(), 0);
    }
}
