use crate::model::{Block, BlockHash, BlockHeight, Hashable};
use crate::utils::db::{Db, Iter, KeyMaker};
use anyhow::Result;
use tracing::{error, info};

pub struct BlocksKey(pub BlockHash);
pub struct BlocksOrdKey(pub BlockHeight);

/// BlocksKey contains a `BlockHash`
impl KeyMaker for BlocksKey {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{}", hex::encode(&self.0.inner));
        writer.as_str()
    }
}

/// BlocksKey contains a `BlockHeight`
impl KeyMaker for BlocksOrdKey {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{:08x}", self.0 .0);
        writer.as_str()
    }
}

#[derive(Debug)]
pub struct Blocks {
    db: Db,
}

impl Blocks {
    pub fn new(db: &sled::Db) -> Result<Self> {
        let db = Db::new(db, "blocks_ord", Some("blocks_alt"))?;
        let blocks = Self { db };

        info!("{} block(s) available", blocks.db.len());

        Ok(blocks)
    }

    pub fn len(&self) -> usize {
        self.db.len()
    }

    pub fn put(&mut self, data: Block) -> Result<()> {
        if self.get(data.hash())?.is_some() {
            return Ok(());
        }
        info!("📦 storing block {}", data.height);
        self.db
            .put(BlocksOrdKey(data.height), BlocksKey(data.hash()), &data)?;
        Ok(())
    }

    pub fn get(&mut self, block_hash: BlockHash) -> Result<Option<Block>> {
        self.db.alt_get(BlocksKey(block_hash))
    }

    pub fn last(&self) -> Option<Block> {
        match self.db.ord_last() {
            Ok(block) => block,
            Err(e) => {
                error!("Error getting last block: {:?}", e);
                None
            }
        }
    }

    pub fn last_block_hash(&self) -> Option<BlockHash> {
        self.last().map(|b| b.hash())
    }

    pub fn range(&mut self, min: BlocksOrdKey, max: BlocksOrdKey) -> Iter<Block> {
        self.db.ord_range(min, max)
    }
}
