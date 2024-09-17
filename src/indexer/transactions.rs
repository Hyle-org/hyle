use super::{model::Transaction, store::Store};
use crate::model::{BlockHeight, TransactionData};
use anyhow::{Context, Result};
use std::borrow::Cow;

#[derive(Debug)]
pub struct Transactions {
    store: Store,
    rels: Store,
}

impl Transactions {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            store: Store::new("transactions", db)?,
            rels: Store::new("transactions_rels", db)?,
        })
    }

    pub fn store(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
        tx_hash: &String,
        data: &TransactionData,
    ) -> Result<()> {
        if true {
            self.store.put(
                tx_hash,
                &Transaction {
                    block_height,
                    tx_index,
                    tx_hash: Cow::Borrowed(tx_hash),
                    data: Cow::Borrowed(data),
                },
            )
        } else {
            // FIXME: store the relation in transactions_rels
            // is relevant ? if we know block_height, we can retrieve the whole block
            // and iter through the transactions
            let res = self.store.put(
                tx_hash,
                &Transaction {
                    block_height,
                    tx_index,
                    tx_hash: Cow::Borrowed(tx_hash),
                    data: Cow::Borrowed(data),
                },
            );
            self.rels
                .put(&format!("{}:{}", block_height, tx_index), tx_hash)?;
            res
        }
    }

    pub fn retrieve(&self, key: &str) -> Result<Option<Transaction>> {
        self.store
            .get(key)
            .with_context(|| format!("retrieving key {}", key))
    }
}
