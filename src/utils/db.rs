use anyhow::{Context, Result};
use core::str;
use serde::{de::DeserializeOwned, Serialize};
use sled::Transactional;
use std::{fmt::Write, marker::PhantomData};
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

/// KeyMaker makes it easy to build keys without allocating each time.
pub trait KeyMaker {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str;
}

impl KeyMaker for &str {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        _ = write!(writer, "{}", self);
        writer.as_str()
    }
}

impl KeyMaker for usize {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        let width = std::mem::size_of::<usize>();
        _ = write!(writer, "{:0width$x}", self);
        writer.as_str()
    }
}

impl KeyMaker for u64 {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        _ = write!(writer, "{:08x}", self);
        writer.as_str()
    }
}

impl<T: KeyMaker> KeyMaker for &T {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        KeyMaker::make_key(*self, writer)
    }
}

pub struct NoKey;

impl KeyMaker for NoKey {
    fn make_key<'a>(&self, _writer: &'a mut String) -> &'a str {
        ""
    }
}

/// Contains the `sled::Tree`
#[derive(Debug)]
struct DbTree {
    // This is the name of the `sled::Tree`.
    name: &'static str,
    tree: sled::Tree,
    // Buffer used to build keys.
    key: String,
    // Buffer used to build range keys.
    key_range: String,
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
                key_range: String::new(),
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
                key_range: String::new(),
            },
            alt,
        })
    }

    pub fn len(&self) -> usize {
        self.ord.tree.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get_raw<T: DeserializeOwned>(db: &mut DbTree, key: impl KeyMaker) -> Result<Option<T>> {
        db.key.clear();
        let key = key.make_key(&mut db.key);
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
    pub fn ord_get<T: DeserializeOwned>(&mut self, key: impl KeyMaker) -> Result<Option<T>> {
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
    pub fn ord_range<T: DeserializeOwned>(
        &mut self,
        min: impl KeyMaker,
        max: impl KeyMaker,
    ) -> Iter<T> {
        self.ord.key.clear();
        let min = min.make_key(&mut self.ord.key);
        let max = max.make_key(&mut self.ord.key_range);
        Iter(self.ord.tree.range(min..max), PhantomData)
    }

    /// Create an iterator over tuples of keys and values, where the all the keys starts with the given prefix.
    pub fn ord_scan_prefix<T: DeserializeOwned>(&mut self, prefix: impl KeyMaker) -> Iter<T> {
        self.ord.key.clear();
        let prefix = prefix.make_key(&mut self.ord.key);
        Iter(self.ord.tree.scan_prefix(prefix), PhantomData)
    }

    /// Retrieve a value if it exists.
    /// The key is built by using the `KeyMaker` given when `f` is called.
    pub fn alt_get<T: DeserializeOwned>(&mut self, key: impl KeyMaker) -> Result<Option<T>> {
        Self::get_raw(
            self.alt
                .as_mut()
                .with_context(|| format!("no alternate tree for {}", self.ord.name))?,
            key,
        )
    }

    /// Create a double-ended iterator over tuples of keys and values, where the keys fall within the specified range.
    /// NOTE: the keys are unorederd.
    pub fn alt_range<T: DeserializeOwned>(
        &mut self,
        min: impl KeyMaker,
        max: impl KeyMaker,
    ) -> Option<Iter<T>> {
        self.alt.as_mut().map(|alt| {
            alt.key.clear();
            let min = min.make_key(&mut alt.key);
            let max = max.make_key(&mut alt.key_range);
            Iter(alt.tree.range(min..max), PhantomData)
        })
    }

    /// Create an iterator over tuples of keys and values, where the all the keys starts with the given prefix.
    /// NOTE: the keys are unorederd.
    pub fn alt_scan_prefix<T: DeserializeOwned>(
        &mut self,
        prefix: impl KeyMaker,
    ) -> Option<Iter<T>> {
        self.alt.as_mut().map(|alt| {
            alt.key.clear();
            let prefix = prefix.make_key(&mut alt.key);
            Iter(alt.tree.scan_prefix(prefix), PhantomData)
        })
    }

    /// Insert a key to a new value.
    pub fn put<T: Serialize>(
        &mut self,
        ord_key: impl KeyMaker,
        alt_key: impl KeyMaker,
        data: &T,
    ) -> Result<()> {
        if let Some(alt) = self.alt.as_mut() {
            self.ord.key.clear();
            alt.key.clear();
            let ord_key = ord_key.make_key(&mut self.ord.key);
            let alt_key = alt_key.make_key(&mut alt.key);
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
            self.ord.key.clear();
            let key = ord_key.make_key(&mut self.ord.key);
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

#[cfg(test)]
mod tests {
    use super::{Db, KeyMaker};
    use anyhow::Result;
    use core::str;

    struct TestKeyOrd(usize, usize);

    impl KeyMaker for TestKeyOrd {
        fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
            use std::fmt::Write;
            _ = write!(writer, "{:08x}:{:08x}", self.0, self.1);
            writer.as_str()
        }
    }

    struct TestKeyAlt<'a>(&'a str, usize);

    impl KeyMaker for TestKeyAlt<'_> {
        fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
            use std::fmt::Write;
            _ = write!(writer, "{}:{:08x}", self.0, self.1);
            writer.as_str()
        }
    }

    type TestDataType = [u8; std::mem::size_of::<usize>()];

    #[test]
    fn test_db_noalt() -> Result<()> {
        let tmpdir = tempfile::Builder::new().prefix("tests").tempdir()?;
        let db = sled::open(tmpdir.path().join("db"))?;
        let mut tree = Db::new(&db, "db", None)?;
        assert!(
            tree.is_empty(),
            "calling len on an empty database should return 0"
        );
        let last = tree.ord_last::<TestDataType>()?;
        assert!(
            last.is_none(),
            "calling last on an empty database should return nothing"
        );

        let data = 1usize;
        let ord_key = TestKeyOrd(1, 1);
        let alt_key = TestKeyAlt("test1", 1);
        tree.put(&ord_key, &alt_key, &data.to_be_bytes())?;
        assert!(tree.len() == 1, "after calling put, len should be 1");
        let last = tree.ord_last::<TestDataType>()?;
        assert_eq!(
            last,
            Some(data.to_be_bytes()),
            "last should return the last entry"
        );

        let tmp = tree.ord_get::<TestDataType>(&ord_key)?;
        assert!(tmp.is_some(), "get (ord) should return the entry");
        assert_eq!(
            tmp,
            Some(data.to_be_bytes()),
            "get should return the entry requested"
        );
        let tmp = tree.alt_get::<TestDataType>(&alt_key);
        assert!(tmp.is_err(), "get (alt) should return an error");
        assert_eq!(
            format!("{}", tmp.unwrap_err()),
            "no alternate tree for db",
            "get (alt) should return the entry"
        );
        Ok(())
    }

    #[test]
    fn test_db() -> Result<()> {
        let tmpdir = tempfile::Builder::new().prefix("tests").tempdir()?;
        let db = sled::open(tmpdir.path().join("db"))?;
        let mut tree = Db::new(&db, "db", Some("db_alt"))?;
        assert!(
            tree.is_empty(),
            "calling len on an empty database should return 0"
        );
        let last = tree.ord_last::<TestDataType>()?;
        assert!(
            last.is_none(),
            "calling last on an empty database should return nothing"
        );

        let mut data = 1usize;
        let ord_key = TestKeyOrd(1, 1);
        let alt_key = TestKeyAlt("test1", 1);
        tree.put(&ord_key, &alt_key, &data.to_be_bytes())?;
        assert!(tree.len() == 1, "after calling put, len should be 1");
        let last = tree.ord_last::<TestDataType>()?;
        assert_eq!(
            last,
            Some(data.to_be_bytes()),
            "last should return the last entry"
        );

        let tmp = tree.ord_get::<TestDataType>(&ord_key)?;
        assert!(tmp.is_some(), "get (ord) should return the entry");
        assert_eq!(
            tmp,
            Some(data.to_be_bytes()),
            "get should return the entry requested"
        );

        data = 0;
        let mut key = String::new();
        for path1 in 1..=5 {
            for path2 in 1..=5 {
                data += 1;
                let ord_key = TestKeyOrd(path1, path2);
                let k1 = format!("test{}", data);
                let alt_key = TestKeyAlt(k1.as_str(), path2);
                tree.put(&ord_key, alt_key, &data.to_be_bytes())?;
                let last = tree.ord_last::<TestDataType>()?;
                assert_eq!(
                    last,
                    Some(data.to_be_bytes()),
                    "last should return the last entry"
                );
            }
        }

        assert_eq!(tree.len(), data, "invalid tree length");

        key.clear();
        let min = TestKeyOrd(1, 1);
        let max = TestKeyOrd(5, 5);
        let iter = tree.ord_range::<TestDataType>(&min, &max);
        for (i, item) in iter.enumerate() {
            let elem = item?;
            let ord_key = TestKeyOrd(i / 5 + 1, i % 5 + 1);
            key.clear();
            assert_eq!(
                elem.key(),
                Some(ord_key.make_key(&mut key)),
                "key should match"
            );
            assert_eq!(
                elem.value(),
                Ok((i + 1).to_be_bytes()),
                "data should match {}",
                elem.key().unwrap()
            );
            let k1 = format!("test{}", i + 1);
            let alt_key = TestKeyAlt(k1.as_str(), i % 5 + 1);
            key.clear();
            let alt = tree.alt_get::<TestDataType>(alt_key)?;
            assert!(alt.is_some());
            let alt = alt.unwrap();
            assert_eq!(
                usize::from_be_bytes(alt),
                i + 1,
                "alt_get should return the data"
            );
        }

        key.clear();
        let prefix = KeyMaker::make_key(&2usize, &mut key);
        let iter = tree.ord_scan_prefix::<TestDataType>(prefix);
        let mut found = false;
        for (i, item) in iter.enumerate() {
            let elem = item?;
            key.clear();
            TestKeyOrd(2, i + 1).make_key(&mut key);
            assert_eq!(elem.key(), Some(key.as_str()), "key should match");
            found = true;
        }
        assert!(found, "scan_prefix failed");
        Ok(())
    }
}
