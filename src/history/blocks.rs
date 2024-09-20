use core::str;

use super::store::Store;
use crate::model::{Block, BlockHeight};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use tracing::{debug, info};

#[derive(Deserialize, Debug)]
pub struct BlocksFilter {
    pub limit: Option<usize>,
    pub min: Option<BlockHeight>,
    pub max: Option<BlockHeight>,
}

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
                .put(Block::default())
                .context("writing genesis block")?;
        }

        debug!("{} block(s) available", blocks.store.len());
        info!("last block is {}", blocks.last.as_ref().unwrap().height);

        Ok(blocks)
    }

    pub fn get_latest_height(&self) -> BlockHeight {
        self.last.as_ref().unwrap().height
    }

    #[allow(dead_code)]
    fn get_latest(&self) -> &Block {
        self.last.as_ref().unwrap()
    }

    pub fn put(&mut self, block: Block) -> Result<()> {
        let mut buf = [0_u8; 20];
        let key = super::u64_to_str(block.height.0, &mut buf);
        self.store.put(key, &block)?;
        self.last.replace(block);
        Ok(())
    }

    pub fn get(&self, block_height: BlockHeight) -> Result<Option<Block>> {
        let mut buf = [0_u8; 20];
        let key = super::u64_to_str(block_height.0, &mut buf);
        self.store
            .get(key)
            .with_context(|| format!("retrieving block {}", key))
    }

    pub fn search(&self, filter: &BlocksFilter) -> Result<Vec<Block>> {
        let limit = filter
            .limit
            .map(|l| if l > 50 { 50 } else { l })
            .unwrap_or(50);

        let last_height = self.last.as_ref().unwrap().height;

        let max = filter.max.unwrap_or(last_height);
        if max.0 > last_height.0 {
            bail!("max > maxHeight");
        }

        let mut min = filter.min.unwrap_or(BlockHeight(0));
        if min.0 > max.0 {
            bail!("min > max");
        }

        let limit = match max.0 - min.0 {
            n if n > limit as u64 => {
                min.0 = if limit as u64 > max.0 {
                    0
                } else {
                    max.0 - limit as u64
                };
                (max.0 - min.0) as usize
            }
            n => n as usize,
        };

        let mut min_buf = [0_u8; 20];
        let mut max_buf = [0_u8; 20];
        let kmin = super::u64_to_str(min.0, &mut min_buf);
        let kmax = super::u64_to_str(max.0, &mut max_buf);

        let mut blocks = Vec::with_capacity(limit);
        let mut iter = self.store.tree.range(kmin..kmax);
        loop {
            match iter.next() {
                Some(Ok((k, v))) => {
                    let block = if let Ok(key) = str::from_utf8(&k) {
                        ron::de::from_bytes(&v)
                            .with_context(|| format!("deserializing data of {} from blocks", key))?
                    } else {
                        ron::de::from_bytes(&v).context("deserializing data from blocks")?
                    };
                    blocks.push(block);
                }
                Some(Err(e)) => bail!("iterating on blocks: {}", e),
                None => break Ok(blocks),
            }
        }
    }
}
