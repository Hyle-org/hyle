use super::{
    db::{Db, Iter, KeyMaker},
    model::{Proof, ProofCow},
};
use crate::model::{BlockHeight, ProofTransaction};
use anyhow::Result;
use std::borrow::Cow;
use tracing::info;

/// ProofsKey contains a `BlockHeight` and a `tx_index`
#[derive(Debug, Default)]
pub struct ProofsKey(pub BlockHeight, pub usize);

impl KeyMaker for ProofsKey {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{:08x}:{:08x}", self.0 .0, self.1);
        writer.as_str()
    }
}

fn proof_cow<'a>(
    block_height: BlockHeight,
    tx_index: usize,
    tx_hash: &'a String,
    proof: &'a ProofTransaction,
) -> ProofCow<'a> {
    ProofCow {
        blobs_references: Cow::Borrowed(&proof.blobs_references),
        proof: Cow::Borrowed(&proof.proof),
        block_height,
        tx_index,
        tx_hash: Cow::Borrowed(tx_hash),
    }
}

#[derive(Debug)]
pub struct Proofs {
    db: Db,
}

impl std::ops::Deref for Proofs {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl std::ops::DerefMut for Proofs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}

impl Proofs {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            db: Db::new(db, "proofs_ord", Some("proofs_alt"))?,
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
        data: &ProofTransaction,
    ) -> Result<()> {
        let data = proof_cow(block_height, tx_index, tx_hash, data);
        info!("storing proof {}:{}", block_height, tx_index);
        self.db
            .put(ProofsKey(block_height, tx_index), tx_hash.as_str(), &data)
    }

    pub fn get(&mut self, block_height: BlockHeight, tx_index: usize) -> Result<Option<Proof>> {
        self.db.ord_get(ProofsKey(block_height, tx_index))
    }

    pub fn get_with_hash(&mut self, tx_hash: &str) -> Result<Option<Proof>> {
        self.db.alt_get(tx_hash)
    }

    pub fn last(&self) -> Result<Option<Proof>> {
        self.db.ord_last()
    }

    pub fn range(&mut self, min: ProofsKey, max: ProofsKey) -> Iter<Proof> {
        self.db.ord_range(min, max)
    }

    pub fn scan_prefix(&mut self, prefix: ProofsKey) -> Iter<Proof> {
        self.db.ord_scan_prefix(prefix)
    }
}
