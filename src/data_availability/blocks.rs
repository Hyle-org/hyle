use std::{
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

use crate::model::{Block, BlockHash, BlockHeight, Hashable};
use anyhow::{bail, Error, Result};
use indexmap::IndexMap;
use tracing::{debug, info};

#[derive(Debug)]
pub struct Blocks {
    data: IndexMap<BlockHash, Block>,
    save_path: Option<PathBuf>,
    last_saved_hash: Option<BlockHash>,
}

impl Blocks {
    pub fn new(path: Option<&Path>) -> Result<Self> {
        match path {
            Some(path) => {
                let path = path.to_path_buf();
                // Open the file, read the first 8 bytes as u64 to know how many items are in the file
                match std::fs::File::open(&path) {
                    Ok(mut file) => {
                        let mut written: [u8; 8] = [0; 8];
                        file.read_exact(&mut written)?;
                        let written = u64::from_be_bytes(written);
                        let mut data = IndexMap::new();
                        for _ in 0..written {
                            let block: Block = bincode::serde::decode_from_std_read(
                                &mut file,
                                bincode::config::standard(),
                            )?;
                            data.insert(block.hash(), block);
                        }
                        debug!("📦 loaded {} blocks from disk", data.len());
                        let last_saved_hash = data.last().map(|(_, block)| block.hash());
                        Ok(Self {
                            data,
                            save_path: Some(path),
                            last_saved_hash,
                        })
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Ok(Self {
                                data: IndexMap::new(),
                                save_path: Some(path),
                                last_saved_hash: None,
                            })
                        } else {
                            Err(e.into())
                        }
                    }
                }
            }
            None => Ok(Self {
                data: IndexMap::new(),
                save_path: None,
                last_saved_hash: None,
            }),
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn put(&mut self, data: Block) -> Result<()> {
        if self.get(data.hash()).is_some() {
            return Ok(());
        }
        info!("📦 storing block {}", data.height);
        self.data.insert(data.hash(), data);
        self.save_to_disk()?;
        Ok(())
    }

    pub fn get(&mut self, block_hash: BlockHash) -> Option<Block> {
        self.data.get(&block_hash).cloned()
    }

    pub fn contains(&mut self, block: &Block) -> bool {
        self.get(block.hash()).is_some()
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
    ) -> Result<impl Iterator<Item = (&BlockHash, &Block)>, Error> {
        // Items are in order but we don't know wher they are. Binary search.
        let min = self
            .data
            .binary_search_by(|_, block| block.height.0.cmp(&min.0))
            .map_err(|_| anyhow::anyhow!("No block found in the range {}..{}", min, max))?;
        let max = self
            .data
            .binary_search_by(|_, block| block.height.0.cmp(&(max.0 - 1)))
            .map_err(|_| anyhow::anyhow!("No block found in the range {}..{}", min, max))?;
        let Some(iter) = self.data.get_range(min..max + 1) else {
            bail!("No block found in the range {}..{}", min, max);
        };
        Ok(iter.iter())
    }

    fn save_to_disk(&mut self) -> Result<()> {
        let save_path = match &self.save_path {
            Some(path) => path,
            None => return Ok(()),
        };
        let max = match self.data.last().map(|(_, block)| block.height + 1) {
            Some(max) => max,
            None => return Ok(()),
        };
        let min = self
            .last_saved_hash
            .clone()
            .map(|hash| self.data.get(&hash).unwrap().height + 1)
            .unwrap_or(
                self.data
                    .first()
                    .map(|(_, block)| block.height)
                    .unwrap_or(BlockHeight(0)),
            );
        let hashes: Vec<(BlockHash, Block)> = self
            .range(min, max)?
            .map(|(hash, block)| (hash.clone(), block.clone()))
            .collect();
        info!(
            "📦 saving {} blocks to disk to {}",
            hashes.len(),
            save_path.display()
        );
        let mut file = std::fs::File::options()
            .write(true)
            .read(true)
            .create(true)
            .truncate(false)
            .open(&save_path)?;

        // If the file is empty, save 8 bytes.
        if file.metadata()?.len() == 0 {
            file.write_all(&0u64.to_le_bytes())?;
        } else {
            // Go to the end, we're appending
            file.seek(std::io::SeekFrom::End(0))?;
        }

        for (hash, block) in hashes {
            match bincode::serde::encode_into_std_write(
                &block,
                &mut file,
                bincode::config::standard(),
            ) {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("Error saving block to disk: {:?}", e);
                    break;
                }
            }
            self.last_saved_hash = Some(hash);
        }
        if self.last_saved_hash.is_none() {
            return Ok(());
        }
        let actually_written = self
            .get(self.last_saved_hash.clone().unwrap())
            .unwrap()
            .height
            .0
            - min.0
            + 1;
        debug!("📦 saved {} blocks to disk", actually_written);
        // Go back to the file beginning, read the number of items as u64, write it back
        // TODO: if we error here, we'll need to clean it up when loading the data.
        file.seek(std::io::SeekFrom::Start(0))?;
        let mut currently_written: [u8; 8] = [0; 8];
        file.read_exact(&mut currently_written)?;
        let mut written = u64::from_be_bytes(currently_written);
        written += actually_written;
        file.seek(std::io::SeekFrom::Start(0))?;
        file.write_all(&written.to_be_bytes())?;
        Ok(())
    }
}
