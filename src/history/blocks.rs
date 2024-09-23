use super::db::Db;
use crate::model::{Block, BlockHeight};
use anyhow::{Context, Result};
use tracing::{debug, info};

#[derive(Debug)]
pub struct Blocks {
    db: Db,
    last: Option<Block>,
}

impl std::ops::Deref for Blocks {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl std::ops::DerefMut for Blocks {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
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

    pub fn put(&mut self, data: &Block) -> Result<()> {
        info!("storing block {}", data.height);
        self.db.put(|km| km.add(data.height), |_| {}, data)
    }

    pub fn get(&mut self, block_height: BlockHeight) -> Result<Option<Block>> {
        self.db.ord_get(|km| {
            km.add(block_height);
        })
    }

    pub fn last(&self) -> &Block {
        self.last.as_ref().unwrap()
    }
}
