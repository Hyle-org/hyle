use super::db::{Db, Iter, KeyMaker};
use crate::model::{Block, BlockHeight};
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use tracing::{debug, info};

#[derive(Debug, Default)]
pub struct BlocksKey(pub BlockHeight);

/// BlocksKey contains a `BlockHeight`
impl KeyMaker for BlocksKey {
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
        let db = Db::new(db, "blocks_ord", None)?;
        let mut blocks = Self {
            last: db.ord_last()?,
            db,
        };

        if blocks.last.is_none() {
            blocks
                .put(&Block::default())
                .context("writing genesis block")?;
        }

        debug!("{} block(s) available", blocks.db.len());
        info!("last block is {}", blocks.last.as_ref().unwrap().height);

        Ok(blocks)
    }

    pub fn len(&self) -> usize {
        self.db.len()
    }

    pub fn put(&mut self, data: &Block) -> Result<()> {
        info!("storing block {}", data.height);
        self.db
            .put(BlocksKey(data.height), BlocksKey::default(), data)
    }

    pub fn get(&mut self, block_height: BlockHeight) -> Result<Option<Block>> {
        self.db.ord_get(BlocksKey(block_height))
    }

    pub fn last(&self) -> &Block {
        self.last.as_ref().unwrap()
    }

    pub fn range<T: DeserializeOwned>(&mut self, min: BlocksKey, max: BlocksKey) -> Iter<T> {
        self.db.ord_range(min, max)
    }

    pub fn scan_prefix<T: DeserializeOwned>(&mut self, prefix: BlocksKey) -> Iter<T> {
        self.db.ord_scan_prefix(prefix)
    }
}
