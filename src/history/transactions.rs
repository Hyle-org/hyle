use super::{
    model::{Transaction, TransactionCow},
    store::Store,
};
use crate::model::{BlockHeight, TransactionData};
use anyhow::{bail, Context, Result};
use core::str;
use serde::Deserialize;
use std::borrow::Cow;

#[derive(Deserialize, Debug)]
pub struct TransactionsFilter {
    pub min: Option<usize>,
    pub max: Option<usize>,
    pub limit: Option<usize>,
}

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

    pub fn put(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
        tx_hash: &String,
        data: &TransactionData,
    ) -> Result<()> {
        // self.store.put(
        //     tx_hash,
        //     &TransactionCow {
        //         block_height,
        //         tx_index,
        //         tx_hash: Cow::Borrowed(tx_hash),
        //         data: Cow::Borrowed(data),
        //     },
        // )

        // FIXME: store the relation in transactions_rels
        // is relevant ? if we know block_height, we can retrieve the whole block
        // and iter through the transactions
        let res = self.store.put(
            tx_hash,
            &TransactionCow {
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

    pub fn get(&self, key: &str) -> Result<Option<Transaction>> {
        self.store
            .get(key)
            .with_context(|| format!("retrieving key {}", key))
    }

    pub fn get_with_height_and_index(
        &self,
        block_height: BlockHeight,
        tx_index: usize,
    ) -> Result<Option<Transaction>> {
        let key = format!("{}:{}", block_height, tx_index);
        match self
            .rels
            .get::<String>(&key)
            .with_context(|| format!("retrieving key {}", key))
        {
            Ok(Some(tx_hash)) => self.get(&tx_hash),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn search(&self, filter: &TransactionsFilter) -> Result<Vec<Transaction>> {
        let limit = filter
            .limit
            .map(|l| if l > 50 { 50 } else { l })
            .unwrap_or(50);
        let mut transactions = Vec::with_capacity(limit);

        let mut iter = self.store.tree.range(""..);

        loop {
            match iter.next() {
                Some(Ok((k, v))) => {
                    let contract = if let Ok(key) = str::from_utf8(&k) {
                        ron::de::from_bytes(&v).with_context(|| {
                            format!("deserializing data of {} from transactions", key)
                        })?
                    } else {
                        ron::de::from_bytes(&v).context("deserializing data from transactions")?
                    };
                    transactions.push(contract);
                }
                Some(Err(e)) => bail!("iterating on transactions: {}", e),
                None => break Ok(transactions),
            }
        }
    }
}
