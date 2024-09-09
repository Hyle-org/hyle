use std::collections::HashMap;

use crate::model::{ContractName, TxHash};

use super::model::UnsettledTransaction;

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

    pub fn add(&mut self, tx: UnsettledTransaction) {
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
}
