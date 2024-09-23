use super::{
    db::Db,
    model::{Transaction, TransactionCow},
};
use crate::model::{BlockHeight, TransactionData};
use anyhow::Result;
use core::str;
use std::borrow::Cow;
use tracing::info;

pub fn transaction_cow<'a>(
    block_height: BlockHeight,
    tx_index: usize,
    tx_hash: &'a String,
    data: &'a TransactionData,
) -> TransactionCow<'a> {
    TransactionCow {
        block_height,
        tx_index,
        tx_hash: Cow::Borrowed(tx_hash),
        data: Cow::Borrowed(data),
    }
}

#[derive(Debug)]
pub struct Transactions {
    db: Db,
}

impl std::ops::Deref for Transactions {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl std::ops::DerefMut for Transactions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}

impl Transactions {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            db: Db::new(db, "transactions_ord", Some("transactions_rels"))?,
        })
    }

    pub fn put(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
        tx_hash: &String,
        data: &TransactionData,
    ) -> Result<()> {
        let tx = transaction_cow(block_height, tx_index, tx_hash, data);
        info!("storing tx {}:{}", block_height, tx_index);
        self.db.put(
            |km| {
                km.add(block_height);
                km.add(tx_index);
            },
            |km| km.add(tx_hash),
            &tx,
        )
    }

    pub fn get(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
    ) -> Result<Option<Transaction>> {
        self.db.ord_get(|km| {
            km.add(block_height);
            km.add(tx_index);
        })
    }

    pub fn get_with_hash(&mut self, tx_hash: &str) -> Result<Option<Transaction>> {
        self.db.alt_get(|km| km.add(tx_hash))
    }

    pub fn last(&self) -> Result<Option<Transaction>> {
        self.db.ord_last()
    }
}
