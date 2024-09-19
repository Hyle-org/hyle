use super::{
    model::{Contract, ContractCow},
    store::Store,
};
use crate::model::RegisterContractTransaction;
use anyhow::{bail, Context, Result};
use core::str;
use serde::Deserialize;
use std::borrow::Cow;

#[derive(Debug)]
pub struct Contracts {
    store: Store,
}

#[derive(Deserialize, Debug)]
pub struct ContractsFilter {
    pub limit: Option<usize>,
}

impl Contracts {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            store: Store::new("contracts", db)?,
        })
    }

    pub fn put(&mut self, tx_hash: &String, contract: &RegisterContractTransaction) -> Result<()> {
        let key = format!("{}:{}", contract.contract_name, tx_hash);
        self.store.put(
            // &contract.contract_name.0, ?
            &key,
            &ContractCow {
                contract_name: Cow::Borrowed(&contract.contract_name),
                owner: Cow::Borrowed(&contract.owner),
                program_id: Cow::Borrowed(&contract.program_id),
                verifier: Cow::Borrowed(&contract.verifier),
                state_digest: Cow::Borrowed(&contract.state_digest),
            },
        )
    }

    pub fn get(&self, contract: &str, tx_hash: &str) -> Result<Option<Contract>> {
        let key = format!("{}:{}", contract, tx_hash);
        self.store
            .get(&key)
            .with_context(|| format!("retrieving contract {}", contract))
    }

    pub fn search(&self, filter: &ContractsFilter) -> Result<Vec<Contract>> {
        let limit = filter
            .limit
            .map(|l| if l > 50 { 50 } else { l })
            .unwrap_or(50);
        let mut contracts = Vec::with_capacity(limit);
        let mut iter = self.store.tree.range(""..);
        loop {
            match iter.next() {
                Some(Ok((k, v))) => {
                    let contract = if let Ok(key) = str::from_utf8(&k) {
                        ron::de::from_bytes(&v).with_context(|| {
                            format!("deserializing data of {} from contracts", key)
                        })?
                    } else {
                        ron::de::from_bytes(&v).context("deserializing data from contracts")?
                    };
                    contracts.push(contract);
                }
                Some(Err(e)) => bail!("iterating on contracts: {}", e),
                None => break Ok(contracts),
            }
        }
    }

    pub fn search_by_name(&self, name: &str, filter: &ContractsFilter) -> Result<Vec<Contract>> {
        let limit = filter
            .limit
            .map(|l| if l > 50 { 50 } else { l })
            .unwrap_or(50);
        let mut contracts = Vec::with_capacity(limit);
        let mut iter = self.store.tree.scan_prefix(name);
        loop {
            match iter.next() {
                Some(Ok((k, v))) => {
                    let contract = if let Ok(key) = str::from_utf8(&k) {
                        ron::de::from_bytes(&v).with_context(|| {
                            format!("deserializing data of {} from contracts", key)
                        })?
                    } else {
                        ron::de::from_bytes(&v).context("deserializing data from contracts")?
                    };
                    contracts.push(contract);
                }
                Some(Err(e)) => bail!("iterating on contracts: {}", e),
                None => break Ok(contracts),
            }
        }
    }
}
