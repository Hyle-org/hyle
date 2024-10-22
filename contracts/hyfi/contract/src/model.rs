extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::vec::Vec;
use alloc::{string::String, vec};
use anyhow::{bail, Error};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};

#[derive(Serialize, Deserialize, Debug, Clone, Hash, bincode::Encode, bincode::Decode)]
pub struct Account {
    name: String,
    balance: u64,
}

impl Account {
    pub fn new(name: String, balance: u64) -> Self {
        Account { name, balance }
    }

    pub fn send(&mut self, amount: u64) -> Result<(), Error> {
        if self.balance >= amount {
            self.balance -= amount;
            Ok(())
        } else {
            bail!("Not enough funds in account '{}'", self.name);
        }
    }

    pub fn receive(&mut self, amount: u64) {
        self.balance += amount;
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, Hash, bincode::Encode, bincode::Decode)]
pub struct Balances {
    accounts: Vec<Account>,
    pub collected_fee: u64,
}
impl Default for Balances {
    fn default() -> Self {
        Balances {
            accounts: vec![Account::new("faucet".to_owned(), 1_000_000)],
            collected_fee: 0,
        }
    }
}
impl TryFrom<sdk::StateDigest> for Balances {
    type Error = Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        let (balances, _) = bincode::decode_from_slice(&state.0, bincode::config::standard())
            .map_err(|_| anyhow::anyhow!("Could not decode start height"))?;
        Ok(balances)
    }
}

impl Balances {
    pub fn as_state(&self) -> sdk::StateDigest {
        sdk::StateDigest(
            bincode::encode_to_vec(self, bincode::config::standard())
                .expect("Failed to encode Balances"),
        )
    }

    pub fn add_account(&mut self, account: Account) {
        self.accounts.push(account);
    }

    pub fn get_account_index(&self, name: &str) -> Option<usize> {
        self.accounts.iter().position(|acc| acc.name == name)
    }

    pub fn get_balance(&self, name: &str) -> Result<u64, Error> {
        match self.get_account_index(name) {
            Some(index) => Ok(self.accounts[index].balance),
            None => bail!("Account '{}' does not exist", name),
        }
    }

    pub fn send(&mut self, from: &str, to: &str, amount: u64) -> Result<(), Error> {
        let from_index = match self.get_account_index(from) {
            Some(index) => index,
            None => bail!("Account '{}' does not exist", from),
        };

        self.accounts[from_index].send(amount)?;

        let to_index = match self.get_account_index(to) {
            Some(index) => index,
            None => {
                self.add_account(Account::new(to.to_owned(), 0));
                self.accounts.len() - 1
            }
        };

        self.accounts[to_index].receive(amount);
        Ok(())
    }

    pub fn mint(&mut self, to: &str, amount: u64) -> Result<(), Error> {
        let faucet_account_index = match self.get_account_index("faucet") {
            Some(index) => index,
            None => bail!("Account faucet does not exist. This should not happen"),
        };

        self.accounts[faucet_account_index].send(amount)?;

        let to_index = match self.get_account_index(to) {
            Some(index) => index,
            None => {
                self.add_account(Account::new(to.to_owned(), 0));
                self.accounts.len() - 1
            }
        };
        self.accounts[to_index].receive(amount);
        Ok(())
    }

    pub fn pay_fees(&mut self, from: &str, amount: u64) -> Result<(), Error> {
        let from_index = match self.get_account_index(from) {
            Some(index) => index,
            None => bail!("Account '{}' does not exist", from),
        };
        self.accounts[from_index].send(amount)?;
        self.collected_fee += amount;
        Ok(())
    }

    pub fn hash(&self) -> Vec<u8> {
        // TODO: maybe we could do something smarter here to allow Merkle inclusion proof
        let mut hasher = Sha3_256::new();
        for account in &self.accounts {
            hasher.update(account.name.as_bytes());
            hasher.update(account.balance.to_be_bytes());
        }
        return hasher.finalize().as_slice().to_owned();
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ContractInput {
    pub balances: Balances,
    pub tx_hash: String,
    pub blobs: Vec<Vec<u8>>,
    pub index: usize,
}

#[derive(Serialize, Deserialize, Encode, Decode, Debug, Clone)]
pub enum ContractFunction {
    Transfer {
        from: String,
        to: String,
        amount: u64,
    },
    Mint {
        to: String,
        amount: u64,
    },
    PayFees {
        from: String,
        amount: u64,
    },
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
