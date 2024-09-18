use super::{
    model::{Blob, BlobCow},
    store::Store,
};
use crate::model::{self, BlockHeight, Identity};
use anyhow::{bail, Context, Result};
use core::str;
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

    pub fn put(
        &mut self,
        block_height: BlockHeight,
        transaction_index: usize,
        tx_hash: &String,
        blob_index: usize,
        tx_identity: &Identity,
        blob: &model::Blob,
    ) -> Result<()> {
        // let key = format!("{}:{}", tx_hash, blob_index);
        let key = format!("{}:{}:{}", block_height, transaction_index, blob_index);
        self.store.put(
            &key,
            &BlobCow {
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

    pub fn get(&self, tx_hash: &str, blob_index: usize) -> Result<Option<Blob>> {
        let key = format!("{}:{}", tx_hash, blob_index);
        self.store
            .get(&key)
            .with_context(|| format!("retrieving key {}", key))
    }

    pub fn get_from_tx_hash(&self, tx_hash: &str) -> Result<Vec<Blob>> {
        let mut blobs = Vec::new();
        let mut iter = self.store.scan(tx_hash);
        loop {
            match iter.next() {
                Some(Ok((k, v))) => {
                    let blob = if let Ok(key) = str::from_utf8(&k) {
                        ron::de::from_bytes(&v)
                            .with_context(|| format!("deserializing data of {} from blobs", key))?
                    } else {
                        ron::de::from_bytes(&v).context("deserializing data from blobs")?
                    };
                    blobs.push(blob);
                }
                Some(Err(e)) => bail!("iterating on blobs: {}", e),
                None => break Ok(blobs),
            }
        }
    }
}
