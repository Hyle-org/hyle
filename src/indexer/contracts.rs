use super::{
    db::{Db, Iter},
    model::{Contract, ContractCow},
};
use crate::{
    indexer::db::NoKey,
    model::{BlockHeight, RegisterContractTransaction},
};
use anyhow::Result;
use core::str;
use std::borrow::Cow;
use tracing::info;

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
            db: Db::new(db, "contracts_ord", None)?,
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
        self.db.put(data.contract_name.0.as_str(), NoKey, &data)
    }

    pub fn get(&mut self, name: &str) -> Result<Option<Contract>> {
        self.db.ord_get(name)
    }

    pub fn all(&mut self) -> Iter<Contract> {
        self.db.ord_range("", "\x7f")
    }

    pub fn scan_prefix(&mut self, prefix: &str) -> Iter<Contract> {
        self.db.ord_scan_prefix(prefix)
    }
}
