use std::path::Path;

use crate::model::{BlockHash, BlockHeight, Hashable, SignedBlock};
use anyhow::Result;
use indexmap::IndexMap;
use tracing::info;

#[derive(Debug)]
pub struct Blocks {
    data: IndexMap<BlockHash, SignedBlock>,
}

pub struct FakeSledItem<'a>(&'a SignedBlock);

impl FakeSledItem<'_> {
    pub fn value(&self) -> Option<&SignedBlock> {
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

    pub fn put(&mut self, data: SignedBlock) -> Result<()> {
        if self.contains(&data) {
            return Ok(());
        }
        info!("📦 storing block {}", data.height());
        self.data.insert(data.hash(), data);
        Ok(())
    }

    pub fn get(&mut self, block_hash: BlockHash) -> Result<Option<SignedBlock>> {
        Ok(self.data.get(&block_hash).cloned())
    }

    pub fn contains(&mut self, block: &SignedBlock) -> bool {
        self.get(block.hash()).unwrap_or(None).is_some()
    }

    pub fn last(&self) -> Option<SignedBlock> {
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
            .binary_search_by(|_, block| block.height().0.cmp(&min.0))
        else {
            return Box::new(::std::iter::empty());
        };
        let Ok(max) = self
            .data
            .binary_search_by(|_, block| block.height().0.cmp(&(max.0 - 1)))
        else {
            return Box::new(::std::iter::empty());
        };
        let Some(iter) = self.data.get_range(min..max + 1) else {
            return Box::new(::std::iter::empty());
        };
        Box::new(iter.values().map(|block| Ok(FakeSledItem(block))))
    }
}
