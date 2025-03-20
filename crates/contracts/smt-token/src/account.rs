use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sparse_merkle_tree::{default_store::DefaultStore, traits::Value, SparseMerkleTree, H256};

use crate::{utils::SHA256Hasher, FAUCET_ID, TOTAL_SUPPLY};

pub struct AccountSMT(pub SparseMerkleTree<SHA256Hasher, Account, DefaultStore<Account>>);

impl Default for AccountSMT {
    fn default() -> Self {
        let mut accounts = SparseMerkleTree::default();
        let faucet_account = Account {
            address: FAUCET_ID.to_string(),
            balance: TOTAL_SUPPLY,
            allowances: BTreeMap::new(),
        };
        let faucet_key = faucet_account.get_key();
        accounts
            .update(faucet_key, faucet_account)
            .expect("Failed to initialize faucet account");

        AccountSMT(accounts)
    }
}

#[derive(
    Debug, Default, Clone, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize,
)]
pub struct Account {
    pub address: String,
    pub balance: u128,
    pub allowances: BTreeMap<String, u128>,
}

impl Account {
    pub fn new(address: String, balance: u128) -> Self {
        Account {
            address,
            balance,
            allowances: BTreeMap::new(),
        }
    }

    pub fn get_key(&self) -> H256 {
        Account::compute_key(self.address.clone())
    }

    pub fn compute_key(address: String) -> H256 {
        let mut hasher = Sha256::new();
        hasher.update(address.as_bytes());
        let result = hasher.finalize();
        let mut h = [0u8; 32];
        h.copy_from_slice(&result);
        H256::from(h)
    }

    pub fn update_allowances(&mut self, spender: String, amount: u128) {
        self.allowances.insert(spender, amount);
    }
}

impl Value for Account {
    fn to_h256(&self) -> H256 {
        if self.balance == 0 {
            return H256::zero();
        }

        let serialized = borsh::to_vec(self).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&serialized);
        let result = hasher.finalize();
        let mut h = [0u8; 32];
        h.copy_from_slice(&result);
        H256::from(h)
    }

    fn zero() -> Self {
        Default::default()
    }
}
