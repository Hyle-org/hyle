use super::{model::Contract, store::Store};
use crate::model::RegisterContractTransaction;
use anyhow::{Context, Result};
use std::borrow::Cow;

#[derive(Debug)]
pub struct Contracts {
    store: Store,
}

impl Contracts {
    pub fn new(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            store: Store::new("contracts", db)?,
        })
    }

    pub fn store(&mut self, contract: &RegisterContractTransaction) -> Result<()> {
        self.store.put(
            &contract.contract_name.0,
            &Contract {
                contract_name: Cow::Borrowed(&contract.contract_name),
                owner: Cow::Borrowed(&contract.owner),
                program_id: Cow::Borrowed(&contract.program_id),
                verifier: Cow::Borrowed(&contract.verifier),
                state_digest: Cow::Borrowed(&contract.state_digest),
            },
        )
    }

    pub fn retrieve(&self, contract: &str) -> Result<Option<Contract>> {
        self.store
            .get(contract)
            .with_context(|| format!("retrieving contract {}", contract))
    }
}
