use anyhow::{Context, Result};
use core::str;
use serde::{de::DeserializeOwned, Serialize};
use sled::Transactional;
use std::marker::PhantomData;
use tracing::debug;

/// Tiny wrapper around sled's iterator item.
pub struct Item<T: DeserializeOwned>(sled::IVec, sled::IVec, PhantomData<T>);

impl<T: DeserializeOwned> Item<T> {
    /// Wrap a sled Iterator Item in order to provide convenience methods.
    pub fn wrap(o: Option<sled::Result<(sled::IVec, sled::IVec)>>) -> Option<sled::Result<Self>> {
        match o {
            Some(Ok((k, v))) => Some(Ok(Item(k, v, PhantomData))),
            Some(Err(e)) => Some(Err(e)),
            None => None,
        }
    }

    /// convenience method to retrieve the key from a `sled::IVec`.
    pub fn key(&self) -> Option<&str> {
        str::from_utf8(&self.0).ok()
    }

    /// convenience method to retrieve and deserialize the data from a `sled::IVec`.
    pub fn value(&self) -> ron::error::SpannedResult<T> {
        ron::de::from_bytes(&self.1)
    }
}

/// Tiny wrapper around sled's iterator.
pub struct Iter<T: DeserializeOwned>(sled::Iter, PhantomData<T>);

impl<T: DeserializeOwned> Iterator for Iter<T> {
    type Item = sled::Result<Item<T>>;

    fn next(&mut self) -> Option<Self::Item> {
        Item::wrap(self.0.next())
    }

    fn last(self) -> Option<Self::Item> {
        Item::wrap(self.0.last())
    }
}

impl<T: DeserializeOwned> DoubleEndedIterator for Iter<T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        Item::wrap(self.0.next_back())
    }
}

/// KeyMaker makes it easy to build keys from multiple parts.
pub struct KeyMaker<'a> {
    first: bool,
    writer: &'a mut String,
}

impl<'a> KeyMaker<'a> {
    /// Allows building keys part by part.
    /// A key is built from all the parts joined together with a colon (:).
    pub fn add<T: std::fmt::Display>(&mut self, elem: T) {
        use std::fmt::Write;
        if self.first {
            _ = write!(&mut self.writer, "{:020}", elem);
            self.first = false
        } else {
            _ = write!(&mut self.writer, ":{:020}", elem);
        }
    }
}

fn make_key<F: FnOnce(&'_ mut KeyMaker<'_>)>(writer: &mut String, f: F) -> &str {
    writer.clear();
    f(&mut KeyMaker {
        first: true,
        writer,
    });
    writer.as_str()
}

/// Contains the `sled::Tree`
#[derive(Debug)]
struct DbTree {
    // This is the name of the `sled::Tree`.
    name: &'static str,
    tree: sled::Tree,
    // Buffer used by `KeyMaker` in order to build keys.
    key: String,
}

/// Db contains the ordered sled::Tree and an optional unordered tree.
/// For example, a transaction can be stored in 2 ways:
/// - using the height and the transaction index from the block it originates from. (ordered).
/// - using the transaction hash (unordered.)
#[derive(Debug)]
pub struct Db {
    // ordered keys (i.e: block height)
    ord: DbTree,
    // unordered keys (i.e: transaction hash)
    alt: Option<DbTree>,
}

impl Db {
    /// Construct a new ordered database with the given name and an optional unordered database.
    pub fn new(
        db: &sled::Db,
        ord_name: &'static str,
        alt_name: Option<&'static str>,
    ) -> Result<Self> {
        let alt = if let Some(name) = alt_name {
            Some(DbTree {
                name,
                tree: db
                    .open_tree(name)
                    .with_context(|| format!("opening {} database", name))?,
                key: String::new(),
            })
        } else {
            None
        };
        Ok(Self {
            ord: DbTree {
                name: ord_name,
                tree: db
                    .open_tree(ord_name)
                    .with_context(|| format!("opening {} database", ord_name))?,
                key: String::new(),
            },
            alt,
        })
    }

    pub fn len(&self) -> usize {
        self.ord.tree.len()
    }

    fn get_raw<T: DeserializeOwned, F: FnOnce(&mut KeyMaker)>(
        db: &mut DbTree,
        key: F,
    ) -> Result<Option<T>> {
        let key = make_key(&mut db.key, key);
        let some_ivec = db
            .tree
            .get(key)
            .with_context(|| format!("retrieving data for {} in {}", key, db.name))?;

        if let Some(ivec) = some_ivec.as_ref() {
            ron::de::from_bytes(ivec)
                .with_context(|| format!("deserializing data for {} in {}", key, db.name))
                .map(Some)
        } else {
            Ok(None)
        }
    }

    /// Retrieve a value if it exists.
    /// The key is built by using the `KeyMaker` given when `f` is called.
    pub fn ord_get<T: DeserializeOwned, F: FnOnce(&mut KeyMaker)>(
        &mut self,
        key: F,
    ) -> Result<Option<T>> {
        Self::get_raw(&mut self.ord, key)
    }

    /// Retrieve the last value or `None` if the `Tree` is empty.
    pub fn ord_last<T: DeserializeOwned>(&self) -> Result<Option<T>> {
        if let Some((key, value)) = self
            .ord
            .tree
            .last()
            .with_context(|| format!("retrieving last entry for {}", self.ord.name))?
            .as_ref()
        {
            ron::de::from_bytes(value)
                .with_context(|| {
                    if let Ok(key) = str::from_utf8(key) {
                        format!("deserializing data for {} in {}", key, self.ord.name)
                    } else {
                        format!("deserializing data for last entry in {}", self.ord.name)
                    }
                })
                .map(Some)
        } else {
            Ok(None)
        }
    }

    /// Create a double-ended iterator over tuples of keys and values, where the keys fall within the specified range.
    pub fn ord_range<T: DeserializeOwned, F: FnOnce(&mut KeyMaker), G: FnOnce(&mut KeyMaker)>(
        &mut self,
        min: F,
        max: G,
    ) -> Iter<T> {
        let min = make_key(&mut self.ord.key, min).to_string();
        let max = make_key(&mut self.ord.key, max);
        Iter(self.ord.tree.range(min.as_str()..max), PhantomData)
    }

    /// Create an iterator over tuples of keys and values, where the all the keys starts with the given prefix.
    pub fn ord_scan_prefix<T: DeserializeOwned, F: FnOnce(&mut KeyMaker)>(
        &mut self,
        prefix: F,
    ) -> Iter<T> {
        let prefix = make_key(&mut self.ord.key, prefix);
        Iter(self.ord.tree.scan_prefix(prefix), PhantomData)
    }

    /// Retrieve a value if it exists.
    /// The key is built by using the `KeyMaker` given when `f` is called.
    pub fn alt_get<T: DeserializeOwned, F: FnOnce(&mut KeyMaker)>(
        &mut self,
        key: F,
    ) -> Result<Option<T>> {
        Self::get_raw(
            self.alt
                .as_mut()
                .with_context(|| format!("no alternate tree for {}", self.ord.name))?,
            key,
        )
    }

    /// Create a double-ended iterator over tuples of keys and values, where the keys fall within the specified range.
    /// NOTE: the keys are unorederd.
    pub fn alt_range<T: DeserializeOwned, F: FnOnce(&mut KeyMaker), G: FnOnce(&mut KeyMaker)>(
        &mut self,
        min: F,
        max: G,
    ) -> Option<Iter<T>> {
        self.alt.as_mut().map(|alt| {
            let min = make_key(&mut alt.key, min).to_string();
            let max = make_key(&mut alt.key, max);
            Iter(alt.tree.range(min.as_str()..max), PhantomData)
        })
    }

    /// Create an iterator over tuples of keys and values, where the all the keys starts with the given prefix.
    /// NOTE: the keys are unorederd.
    pub fn alt_scan_prefix<T: DeserializeOwned, F: FnOnce(&mut KeyMaker)>(
        &mut self,
        prefix: F,
    ) -> Option<Iter<T>> {
        self.alt.as_mut().map(|alt| {
            let prefix = make_key(&mut alt.key, prefix);
            Iter(alt.tree.scan_prefix(prefix), PhantomData)
        })
    }

    /// Insert a key to a new value.
    pub fn put<T: Serialize, F: FnOnce(&mut KeyMaker), G: FnOnce(&mut KeyMaker)>(
        &mut self,
        ord_key: F,
        alt_key: G,
        data: &T,
    ) -> Result<()> {
        if let Some(alt) = self.alt.as_mut() {
            let ord_key = make_key(&mut self.ord.key, ord_key);
            let alt_key = make_key(&mut alt.key, alt_key);
            let serialized_data = ron::ser::to_string(data).with_context(|| {
                format!("serializing data for {} in {}", ord_key, self.ord.name)
            })?;
            (&self.ord.tree, &alt.tree)
                .transaction(|(ord, alt)| {
                    _ = ord.insert(ord_key, serialized_data.as_str())?;
                    _ = alt.insert(alt_key, serialized_data.as_str())?;
                    Ok(())
                        as std::result::Result<(), sled::transaction::ConflictableTransactionError>
                })
                .with_context(|| {
                    format!("transaction failed for {} in {}", ord_key, self.ord.name)
                })?;
            self.ord
                .tree
                .flush()
                .with_context(|| format!("flushing {}", self.ord.name))?;
            alt.tree
                .flush()
                .with_context(|| format!("fluhing {}", alt.name))?;
            debug!("{} written to {}", ord_key, self.ord.name);
        } else {
            let key = make_key(&mut self.ord.key, ord_key);
            let serialized_data = ron::ser::to_string(data)
                .with_context(|| format!("serializing data for {} in {}", key, self.ord.name))?;
            self.ord
                .tree
                .insert(key, serialized_data.as_str())
                .with_context(|| format!("inserting data for {} to {}", key, self.ord.name))?;
            self.ord
                .tree
                .flush()
                .with_context(|| format!("flushing {}", self.ord.name))?;
            debug!("{} written to {}", key, self.ord.name);
        }
        Ok(())
    }
}
