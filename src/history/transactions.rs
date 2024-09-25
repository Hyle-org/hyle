use super::{
    db::{Db, Iter, KeyMaker},
    model::{Transaction, TransactionCow},
};
use crate::model::{BlockHeight, TransactionData};
use anyhow::Result;
use core::str;
use serde::de::DeserializeOwned;
use std::borrow::Cow;
use tracing::info;

/// TransactionsKey contains a `BlockHeight` and a `tx_index`
#[derive(Debug, Default)]
pub struct TransactionsKey(pub BlockHeight, pub usize);

impl KeyMaker for TransactionsKey {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{:08x}:{:08x}", self.0 .0, self.1);
        writer.as_str()
    }
}

/// TransactionsKeyAlt contains a `tx_hash`
#[derive(Debug, Default)]
pub struct TransactionsKeyAlt<'b>(pub &'b str);

impl<'b> KeyMaker for TransactionsKeyAlt<'b> {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{}", self.0);
        writer.as_str()
    }
}

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

impl Transactions {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            db: Db::new(db, "transactions_ord", Some("transactions_rels"))?,
        })
    }

    pub fn len(&self) -> usize {
        self.db.len()
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
            TransactionsKey(block_height, tx_index),
            TransactionsKeyAlt(tx_hash),
            &tx,
        )
    }

    pub fn get(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
    ) -> Result<Option<Transaction>> {
        self.db.ord_get(TransactionsKey(block_height, tx_index))
    }

    pub fn get_with_hash(&mut self, tx_hash: &str) -> Result<Option<Transaction>> {
        self.db.alt_get(TransactionsKeyAlt(tx_hash))
    }

    pub fn last(&self) -> Result<Option<Transaction>> {
        self.db.ord_last()
    }

    pub fn range<T: DeserializeOwned>(
        &mut self,
        min: TransactionsKey,
        max: TransactionsKey,
    ) -> Iter<T> {
        self.db.ord_range(min, max)
    }

    pub fn scan_prefix<T: DeserializeOwned>(&mut self, prefix: TransactionsKey) -> Iter<T> {
        self.db.ord_scan_prefix(prefix)
    }
}
