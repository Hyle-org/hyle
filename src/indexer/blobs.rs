use super::{model::Blob, store::Store};
use crate::model::{self, BlockHeight, Identity};
use anyhow::{Context, Result};
use std::borrow::Cow;

#[derive(Debug)]
pub struct Blobs {
    store: Store,
}

impl Blobs {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            store: Store::new("blobs", db)?,
        })
    }

    pub fn store(
        &mut self,
        block_height: BlockHeight,
        transaction_index: usize,
        tx_hash: &String,
        blob_index: usize,
        tx_identity: &Identity,
        blob: &model::Blob,
    ) -> Result<()> {
        // TODO: replace current key with blob_hash ?
        let key = format!("{}:{}:{}", block_height, transaction_index, blob_index);
        self.store.put(
            &key,
            &Blob {
                identity: Cow::Borrowed(tx_identity),
                contract_name: Cow::Borrowed(&blob.contract_name),
                data: Cow::Borrowed(&blob.data),
                block_height,
                tx_index: transaction_index,
                tx_hash: Cow::Borrowed(tx_hash),
                blob_index,
            },
        )
    }

    pub fn retrieve(&self, key: &str) -> Result<Option<Blob>> {
        self.store
            .get(key)
            .with_context(|| format!("retrieving key {}", key))
    }
}
