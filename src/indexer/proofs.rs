use super::{model::Proof, store::Store};
use crate::model::{BlobReference, BlockHeight};
use anyhow::{Context, Result};
use std::borrow::Cow;

#[derive(Debug)]
pub struct Proofs {
    store: Store,
}

impl Proofs {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            store: Store::new("proofs", db)?,
        })
    }

    pub fn store(
        &mut self,
        block_height: BlockHeight,
        transaction_index: usize,
        tx_hash: &String,
        blobs_references: &Vec<BlobReference>,
        proof: &Vec<u8>,
    ) -> Result<()> {
        let key = format!("{}:{}", block_height, transaction_index);
        self.store.put(
            &key,
            &Proof {
                blobs_references: Cow::Borrowed(blobs_references),
                proof: Cow::Borrowed(proof),
                block_height,
                tx_index: transaction_index,
                tx_hash: Cow::Borrowed(tx_hash),
            },
        )
    }

    pub fn retrieve(&self, key: &str) -> Result<Option<Proof>> {
        self.store
            .get(key)
            .with_context(|| format!("retrieving key {}", key))
    }
}
