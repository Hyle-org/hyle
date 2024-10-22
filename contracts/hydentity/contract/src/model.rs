extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use anyhow::{bail, Error};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Hash, bincode::Encode, bincode::Decode)]
pub struct Account {
    name: String,
    password: String,
}

impl Account {
    pub fn new(name: String, password: String) -> Self {
        Account { name, password }
    }

    pub fn check_password(&mut self, password: String) -> Result<(), Error> {
        if self.password == password {
            Ok(())
        } else {
            bail!("Wrong password for {}", self.name);
        }
    }
}

#[derive(Default, Clone, Serialize, Deserialize, Debug, Hash, bincode::Encode, bincode::Decode)]
pub struct Identities {
    accounts: Vec<Account>,
}
impl TryFrom<sdk::StateDigest> for Identities {
    type Error = Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        let (balances, _) = bincode::decode_from_slice(&state.0, bincode::config::standard())
            .map_err(|_| anyhow::anyhow!("Could not decode start height"))?;
        Ok(balances)
    }
}

impl Identities {
    pub fn as_state(&self) -> sdk::StateDigest {
        sdk::StateDigest(
            bincode::encode_to_vec(self, bincode::config::standard())
                .expect("Failed to encode Identities"),
        )
    }

    pub fn register(&mut self, account: String, password: String) -> Result<(), Error> {
        self.accounts.push(Account::new(account, password));
        Ok(())
    }

    pub fn check_password(&mut self, account: String, password: String) -> Result<(), Error> {
        for acc in &mut self.accounts {
            if acc.name == account {
                return acc.check_password(password);
            }
        }
        bail!("Account not found");
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ContractInput {
    pub identities: Identities,
    pub tx_hash: String,
    pub blobs: Vec<Vec<u8>>,
    pub index: usize,
}

#[derive(Serialize, Deserialize, Encode, Decode, Debug, Clone)]
pub enum ContractFunction {
    Register { account: String, password: String },
    CheckPassword { account: String, password: String },
}
impl ContractFunction {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let r = bincode::encode_to_vec(self, bincode::config::standard())?;
        Ok(r)
    }

    pub fn decode(data: &[u8]) -> Result<Self, Error> {
        let (v, _) = bincode::decode_from_slice(data, bincode::config::standard())?;
        Ok(v)
    }
}
