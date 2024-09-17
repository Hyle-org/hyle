use super::{
    blobs::Blobs, blocks::Blocks, contracts::Contracts, proofs::Proofs, transactions::Transactions,
};
use anyhow::{Context, Result};
use core::str;
use std::io::{Cursor, Write};

#[derive(Debug)]
pub struct Db {
    // db: sled::Db,
    pub blocks: Blocks,
    pub blobs: Blobs,
    pub proofs: Proofs,
    pub contracts: Contracts,
    pub transactions: Transactions,
}

impl Db {
    pub fn new(path: &str) -> Result<Self> {
        let db = sled::Config::new()
            .use_compression(true)
            .compression_factor(15)
            .path(path)
            .open()
            .context("opening the database")?;
        Ok(Self {
            blocks: Blocks::new(&db)?,
            blobs: Blobs::new(&db)?,
            proofs: Proofs::new(&db)?,
            contracts: Contracts::new(&db)?,
            transactions: Transactions::new(&db)?,
            // db,
        })
    }
}

pub fn u64_to_str(u: u64, buf: &mut [u8]) -> &str {
    let mut cursor = Cursor::new(&mut buf[..]);
    _ = write!(cursor, "{}", u);
    let len = cursor.position() as usize;
    str::from_utf8(&buf[..len]).unwrap()
}
