use super::store::Store;
use crate::model::{Block, BlockHeight};
use anyhow::{Context, Result};
use tracing::{debug, info};

#[derive(Debug)]
pub struct Blocks {
    store: Store,
    pub last: Option<Block>,
}

impl Blocks {
    pub fn new(db: &sled::Db) -> Result<Self> {
        let mut blocks = Self {
            store: Store::new("blocks", db)?,
            last: None,
        };

        blocks.last = blocks.store.last().context("retrieving latest block")?;
        if blocks.last.is_none() {
            blocks
                .store(Block::default())
                .context("writing genesis block")?;
        }

        debug!("{} block(s) available", blocks.store.len());
        info!("last block is {}", blocks.last.as_ref().unwrap().height);

        Ok(blocks)
    }

    pub fn get_latest_height(&self) -> BlockHeight {
        self.last.as_ref().unwrap().height
    }

    fn get_latest(&self) -> &Block {
        self.last.as_ref().unwrap()
    }

    pub fn store(&mut self, block: Block) -> Result<()> {
        let mut buf = [0_u8; 20];
        let key = super::db::u64_to_str(block.height.0, &mut buf);
        self.store.put(key, &block)?;
        self.last.replace(block);
        Ok(())
    }

    pub fn retrieve(&self, block_height: BlockHeight) -> Result<Option<Block>> {
        let mut buf = [0_u8; 20];
        let key = super::db::u64_to_str(block_height.0, &mut buf);
        self.store
            .get(key)
            .with_context(|| format!("retrieving block {}", key))
    }
}
