use super::{
    db::{Db, Iter},
    model::{Contract, ContractCow},
};
use crate::model::{BlockHeight, RegisterContractTransaction};
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

impl std::ops::Deref for Contracts {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl std::ops::DerefMut for Contracts {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}

impl Contracts {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            db: Db::new(db, "contracts_ord", Some("contracts_alt"))?,
        })
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
            |km| {
                km.add(block_height);
                km.add(tx_index);
            },
            |km| {
                km.add(tx_hash);
            },
            &data,
        )
    }

    pub fn get(&mut self, block_height: BlockHeight, tx_index: usize) -> Result<Option<Contract>> {
        self.db.ord_get(|km| {
            km.add(block_height);
            km.add(tx_index);
        })
    }

    pub fn get_with_name(&mut self, name: &str) -> Option<Iter<Contract>> {
        self.db.alt_scan_prefix(|km| {
            km.add(name);
            km.add("")
        })
    }

    pub fn last(&self) -> Result<Option<Contract>> {
        self.db.ord_last()
    }
}
