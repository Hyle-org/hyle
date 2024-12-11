use crate::model::{BlockHeight, Hashable, ProcessedBlock, ProcessedBlockHash};
use crate::utils::db::{Db, Iter, KeyMaker};
use anyhow::Result;
use tracing::{error, info};

pub struct BlocksKey(pub ProcessedBlockHash);
pub struct BlocksOrdKey(pub BlockHeight);

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
pub struct ProcessedBlocks {
    db: Db,
}

impl ProcessedBlocks {
    pub fn new(db: &sled::Db) -> Result<Self> {
        let db = Db::new(db, "blocks_ord", Some("blocks_alt"))?;
        let processed_blocks = Self { db };

        info!("{} processed block(s) available", processed_blocks.db.len());

        Ok(processed_blocks)
    }

    pub fn len(&self) -> usize {
        self.db.len()
    }

    pub fn put(&mut self, processed_block: ProcessedBlock) -> Result<()> {
        if self.get(processed_block.hash())?.is_some() {
            return Ok(());
        }
        info!(
            "📦 storing processed block {}",
            processed_block.block_height
        );
        self.db.put(
            BlocksOrdKey(processed_block.block_height),
            BlocksKey(processed_block.hash()),
            &processed_block,
        )?;
        Ok(())
    }

    pub fn get(&mut self, block_hash: ProcessedBlockHash) -> Result<Option<ProcessedBlock>> {
        self.db.alt_get(BlocksKey(block_hash))
    }

    pub fn contains(&mut self, processed_block: &ProcessedBlock) -> bool {
        self.get(processed_block.hash()).ok().flatten().is_some()
    }

    pub fn last(&self) -> Option<ProcessedBlock> {
        match self.db.ord_last() {
            Ok(processed_block) => processed_block,
            Err(e) => {
                error!("Error getting last processed block: {:?}", e);
                None
            }
        }
    }

    pub fn last_block_hash(&self) -> Option<ProcessedBlockHash> {
        self.last().map(|b| b.hash())
    }

    pub fn range(&mut self, min: BlocksOrdKey, max: BlocksOrdKey) -> Iter<ProcessedBlock> {
        self.db.ord_range(min, max)
    }
}
