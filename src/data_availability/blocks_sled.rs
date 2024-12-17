#![cfg_attr(test, allow(unused))]
use std::path::Path;

use crate::model::{BlockHash, BlockHeight, Hashable, SignedBlock};
use crate::utils::db::{Db, Iter, KeyMaker};
use anyhow::{Context, Result};
use tracing::{error, info};

struct BlocksKey(pub BlockHash);
struct BlocksOrdKey(pub BlockHeight);

/// BlocksKey contains a `BlockHash`
impl KeyMaker for BlocksKey {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        writer.clear();
        _ = write!(writer, "{}", self.0);
        writer.as_str()
    }
}

/// BlocksKey contains a `BlockHeight`
impl KeyMaker for BlocksOrdKey {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        writer.clear();
        _ = write!(writer, "{:08x}", self.0 .0);
        writer.as_str()
    }
}

#[derive(Debug)]
pub struct Blocks {
    db: Db,
}

impl Blocks {
    pub fn new(path: &Path) -> Result<Self> {
        let db = sled::Config::new()
            .use_compression(true)
            .compression_factor(3)
            .path(path)
            .open()
            .context("opening the database")?;

        let db = Db::new(&db, "blocks_ord", Some("blocks_alt"))?;
        let blocks = Self { db };

        info!("{} block(s) available", blocks.db.len());

        Ok(blocks)
    }

    pub fn len(&self) -> usize {
        self.db.len()
    }

    pub fn put(&mut self, block: SignedBlock) -> Result<()> {
        if self.get(block.hash())?.is_some() {
            return Ok(());
        }
        info!("📦 storing block in sled {}", block.height());
        self.db.put(
            BlocksOrdKey(block.height()),
            BlocksKey(block.hash()),
            &block,
        )?;
        Ok(())
    }

    pub fn get(&mut self, block_hash: BlockHash) -> Result<Option<SignedBlock>> {
        self.db.alt_get(BlocksKey(block_hash))
    }

    pub fn contains(&mut self, block: &SignedBlock) -> bool {
        self.get(block.hash()).ok().flatten().is_some()
    }

    pub fn last(&self) -> Option<SignedBlock> {
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

    pub fn range(&mut self, min: BlockHeight, max: BlockHeight) -> Iter<SignedBlock> {
        self.db.ord_range(BlocksOrdKey(min), BlocksOrdKey(max))
    }
}
