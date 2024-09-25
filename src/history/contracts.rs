use super::{
    db::{Db, Iter, KeyMaker},
    model::{Contract, ContractCow},
};
use crate::model::{BlockHeight, RegisterContractTransaction};
use anyhow::Result;
use core::str;
use serde::de::DeserializeOwned;
use std::borrow::Cow;
use tracing::info;

/// ContractsKey contains a `BlockHeight` and a `tx_index`
#[derive(Debug, Default)]
pub struct ContractsKey(pub BlockHeight, pub usize /* tx_index */);

impl KeyMaker for ContractsKey {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{:08x}:{:08x}", self.0 .0, self.1);
        writer.as_str()
    }
}

/// ContractsKeyAlt contains a `name` and a `tx_hash`
#[derive(Debug, Default)]
pub struct ContractsKeyAlt<'b>(pub &'b str, pub &'b str);

impl<'b> KeyMaker for ContractsKeyAlt<'b> {
    fn make_key<'a>(&self, writer: &'a mut String) -> &'a str {
        use std::fmt::Write;
        _ = write!(writer, "{}:{}", self.0, self.1);
        writer.as_str()
    }
}

fn contract_cow<'a>(
    block_height: BlockHeight,
    tx_index: usize,
    tx_hash: &'a String,
    contract: &'a RegisterContractTransaction,
) -> ContractCow<'a> {
    ContractCow {
        owner: Cow::Borrowed(&contract.owner),
        verifier: Cow::Borrowed(&contract.verifier),
        program_id: Cow::Borrowed(&contract.program_id),
        state_digest: Cow::Borrowed(&contract.state_digest),
        contract_name: Cow::Borrowed(&contract.contract_name),
        block_height,
        tx_index,
        tx_hash: Cow::Borrowed(tx_hash),
    }
}

#[derive(Debug)]
pub struct Contracts {
    db: Db,
}

impl Contracts {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            db: Db::new(db, "contracts_ord", Some("contracts_alt"))?,
        })
    }

    pub fn len(&self) -> usize {
        self.db.len()
    }

    pub fn put(
        &mut self,
        block_height: BlockHeight,
        tx_index: usize,
        tx_hash: &String,
        data: &RegisterContractTransaction,
    ) -> Result<()> {
        let data = contract_cow(block_height, tx_index, tx_hash, data);
        info!("storing contract {}:{}", block_height, tx_index);
        self.db.put(
            ContractsKey(block_height, tx_index),
            ContractsKeyAlt(&data.contract_name.0, tx_hash),
            &data,
        )
    }

    pub fn get(&mut self, block_height: BlockHeight, tx_index: usize) -> Result<Option<Contract>> {
        self.db.ord_get(ContractsKey(block_height, tx_index))
    }

    pub fn get_with_name(&mut self, name: &str) -> Option<Iter<Contract>> {
        self.db.alt_scan_prefix(name)
    }

    pub fn last(&self) -> Result<Option<Contract>> {
        self.db.ord_last()
    }

    pub fn range<T: DeserializeOwned>(&mut self, min: ContractsKey, max: ContractsKey) -> Iter<T> {
        self.db.ord_range(min, max)
    }

    pub fn scan_prefix<T: DeserializeOwned>(&mut self, prefix: ContractsKey) -> Iter<T> {
        self.db.ord_scan_prefix(prefix)
    }
}
