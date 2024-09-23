use super::{
    db::Db,
    model::{Blob, BlobCow},
};
use crate::model::{Blob as NodeBlob, BlockHeight, Identity};
use anyhow::Result;
use std::borrow::Cow;
use tracing::info;

fn blob_cow<'a>(
    block_height: BlockHeight,
    tx_index: usize,
    blob_index: usize,
    tx_hash: &'a String,
    tx_identity: &'a Identity,
    blob: &'a NodeBlob,
) -> BlobCow<'a> {
    BlobCow {
        identity: Cow::Borrowed(tx_identity),
        contract_name: Cow::Borrowed(&blob.contract_name),
        data: Cow::Borrowed(&blob.data),
        block_height,
        tx_index,
        blob_index,
        tx_hash: Cow::Borrowed(tx_hash),
    }
}

#[derive(Debug)]
pub struct Blobs {
    db: Db,
}

impl std::ops::Deref for Blobs {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl std::ops::DerefMut for Blobs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}

impl Blobs {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            db: Db::new(db, "blobs_ord", Some("blobs_alt"))?,
        })
    }

    pub fn put(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
        blob_index: usize,
        tx_hash: &String,
        tx_identity: &Identity,
        data: &NodeBlob,
    ) -> Result<()> {
        let data = blob_cow(
            block_height,
            tx_index,
            blob_index,
            tx_hash,
            tx_identity,
            data,
        );
        info!("storing blob {}:{}:{}", block_height, tx_index, blob_index);
        self.db.put(
            |km| {
                km.add(block_height);
                km.add(tx_index);
                km.add(blob_index);
            },
            |km| {
                km.add(tx_hash);
                km.add(blob_index);
            },
            &data,
        )
    }

    pub fn get(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
        blob_index: usize,
    ) -> Result<Option<Blob>> {
        self.db.ord_get(|km| {
            km.add(block_height);
            km.add(tx_index);
            km.add(blob_index);
        })
    }

    pub fn get_with_hash(&mut self, tx_hash: &str, blob_index: usize) -> Result<Option<Blob>> {
        self.db.alt_get(|km| {
            km.add(tx_hash);
            km.add(blob_index);
        })
    }

    pub fn last(&self) -> Result<Option<Blob>> {
        self.db.ord_last()
    }
}
