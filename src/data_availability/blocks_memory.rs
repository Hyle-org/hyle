use std::path::Path;

use crate::model::{Block, BlockHash, BlockHeight, Hashable};
use anyhow::Result;
use indexmap::IndexMap;
use tracing::info;

#[derive(Debug)]
pub struct Blocks {
    data: IndexMap<BlockHash, Block>,
}

pub struct FakeSledItem<'a>(&'a Block);

impl FakeSledItem<'_> {
    pub fn value(&self) -> Option<&Block> {
        Some(self.0)
    }
}

impl Blocks {
    pub fn new(_: &Path) -> Result<Self> {
        Ok(Self {
            data: IndexMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn put(&mut self, data: Block) -> Result<()> {
        if self.contains(&data) {
            return Ok(());
        }
        info!(
            "📦 storing block {} with {} txs",
            data.block_height,
            data.total_txs()
        );
        self.data.insert(data.hash(), data);
        Ok(())
    }

    pub fn get(&mut self, block_hash: BlockHash) -> Result<Option<Block>> {
        Ok(self.data.get(&block_hash).cloned())
    }

    pub fn contains(&mut self, block: &Block) -> bool {
        self.get(block.hash()).unwrap_or(None).is_some()
    }

    pub fn last(&self) -> Option<Block> {
        self.data.last().map(|(_, block)| block.clone())
    }

    pub fn last_block_hash(&self) -> Option<BlockHash> {
        self.last().map(|b| b.hash())
    }

    pub fn range(
        &self,
        min: BlockHeight,
        max: BlockHeight,
    ) -> Box<dyn Iterator<Item = Result<FakeSledItem>> + '_> {
        // Items are in order but we don't know wher they are. Binary search.
        let Ok(min) = self
            .data
            .binary_search_by(|_, block| block.block_height.0.cmp(&min.0))
        else {
            return Box::new(::std::iter::empty());
        };
        let Ok(max) = self
            .data
            .binary_search_by(|_, block| block.block_height.0.cmp(&(max.0 - 1)))
        else {
            return Box::new(::std::iter::empty());
        };
        let Some(iter) = self.data.get_range(min..max + 1) else {
            return Box::new(::std::iter::empty());
        };
        Box::new(iter.values().map(|block| Ok(FakeSledItem(block))))
    }
}
