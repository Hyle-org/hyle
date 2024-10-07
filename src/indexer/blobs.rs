use super::{
    db::{Db, Iter, KeyMaker},
    model::{Blob, BlobCow},
};
use crate::model::{Blob as NodeBlob, BlockHeight, Identity};
use anyhow::Result;
use std::borrow::Cow;
use tracing::info;

/// BlobsKey contains a `BlockHeight` a `tx_index` and a `blob_index`
#[derive(Debug, Default)]
pub struct BlobsKey(pub BlockHeight, pub usize, pub usize);

impl KeyMaker for BlobsKey {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{:08x}:{:08x}:{:08x}", self.0 .0, self.1, self.2);
        writer.as_str()
    }
}

/// BlobsKeyAlt contains a `contract_name`, a `tx_hash` and a `blob_index`
#[derive(Debug, Default)]
pub struct BlobsKeyAlt<'b>(pub &'b str, pub &'b str, pub usize);

impl<'b> KeyMaker for BlobsKeyAlt<'b> {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{}:{}:{:08x}", self.0, self.1, self.2);
        writer.as_str()
    }
}

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

impl Blobs {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            db: Db::new(db, "blobs_ord", Some("blobs_alt"))?,
        })
    }

    pub fn len(&self) -> usize {
        self.db.len()
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
            BlobsKey(block_height, tx_index, blob_index),
            BlobsKeyAlt(data.contract_name.0.as_ref(), tx_hash, blob_index),
            &data,
        )
    }

    pub fn get(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
        blob_index: usize,
    ) -> Result<Option<Blob>> {
        self.db
            .ord_get(BlobsKey(block_height, tx_index, blob_index))
    }

    pub fn get_by_contract_name(&mut self, contract_name: &str) -> Option<Iter<Blob>> {
        self.db.alt_scan_prefix(contract_name)
    }

    pub fn last(&self) -> Result<Option<Blob>> {
        self.db.ord_last()
    }

    pub fn range(&mut self, min: BlobsKey, max: BlobsKey) -> Iter<Blob> {
        self.db.ord_range(min, max)
    }

    pub fn scan_prefix(&mut self, prefix: BlobsKey) -> Iter<Blob> {
        self.db.ord_scan_prefix(prefix)
    }
}
