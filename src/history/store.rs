use anyhow::{Context, Result};
use core::str;
use serde::{de::DeserializeOwned, Serialize};
use tracing::debug;

#[derive(Debug)]
pub struct Store {
    name: &'static str,
    pub tree: sled::Tree,
}

impl Store {
    pub fn new(name: &'static str, db: &sled::Db) -> Result<Self> {
        Ok(Self {
            tree: db
                .open_tree(name)
                .with_context(|| format!("opening {} database", name))?,
            name,
        })
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        self.name
    }

    pub fn len(&self) -> usize {
        self.tree.len()
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let some_ivec = self
            .tree
            .get(key)
            .with_context(|| format!("retrieving {} from {}", key, self.name))?;

        if some_ivec.is_none() {
            return Ok(None);
        }
        ron::de::from_bytes::<T>(&some_ivec.unwrap())
            .with_context(|| format!("deserializing data of {} from {}", key, self.name))
            .map(Some)
    }

    pub fn put<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let serialized_block = ron::ser::to_string(&value)
            .with_context(|| format!("serializing data of {} to {}", key, self.name))?;
        self.tree
            .insert(key, serialized_block.as_bytes())
            .with_context(|| format!("inserting data of {} to {}", key, self.name))?;
        self.tree.flush()?;
        debug!("{} stored successfully to {}", key, self.name);
        Ok(())
    }

    pub fn last<T: DeserializeOwned>(&self) -> Result<Option<T>> {
        if let Some((key, value)) = self
            .tree
            .last()
            .with_context(|| format!("retrieving last object from {}", self.name))?
        {
            return ron::de::from_bytes::<T>(&value)
                .with_context(|| {
                    if let Ok(key) = str::from_utf8(&key) {
                        format!("deserializing data of {} from {}", key, self.name)
                    } else {
                        format!("deserializing latest data from {}", self.name)
                    }
                })
                .map(Some);
        }
        Ok(None)
    }

    #[allow(dead_code)]
    pub fn last_with_key<T: DeserializeOwned>(&self) -> Result<Option<(String, T)>> {
        if let Some((key, value)) = self
            .tree
            .last()
            .with_context(|| format!("retrieving last object from {}", self.name))?
        {
            let key = str::from_utf8(&key)
                .with_context(|| format!("converting last object's key from {}", self.name))?;
            return ron::de::from_bytes::<T>(&value)
                .with_context(|| format!("deserializing data of {} from {}", key, self.name))
                .map(|v| (key.to_string(), v))
                .map(Some);
        }
        Ok(None)
    }

    pub fn scan(&self, prefix: &str) -> sled::Iter {
        self.tree.scan_prefix(prefix)
    }
}
