use crate::model::{Block, BlockHash, BlockHeight, Hashable};
use crate::utils::db::{Db, Iter, KeyMaker};
use anyhow::Result;
use tracing::info;

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
    last: Option<Block>,
}

impl Blocks {
    pub fn new(db: &sled::Db) -> Result<Self> {
        let db = Db::new(db, "blocks_ord", Some("blocks_alt"))?;
        let blocks = Self {
            last: db.ord_last()?,
            db,
        };

        info!("{} block(s) available", blocks.db.len());
        if let Some(last) = blocks.last() {
            info!(
                block_hash = %last.hash(),
                block_height = %last.height,
                "last block is {:?}", last);
        }

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
        self.last.replace(data);
        Ok(())
    }

    pub fn get(&mut self, block_hash: BlockHash) -> Result<Option<Block>> {
        self.db.alt_get(BlocksKey(block_hash))
    }

    pub fn last(&self) -> Option<&Block> {
        self.last.as_ref()
    }

    pub fn last_block_hash(&self) -> Option<BlockHash> {
        self.last.as_ref().map(|b| b.hash())
    }

    pub fn range(&mut self, min: BlocksKey, max: BlocksKey) -> Iter<Block> {
        self.db.ord_range(min, max)
    }

    pub fn scan_prefix(&mut self, prefix: BlocksKey) -> Iter<Block> {
        self.db.ord_scan_prefix(prefix)
    }
}
