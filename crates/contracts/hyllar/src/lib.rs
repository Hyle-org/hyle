use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sdk::erc20::ERC20;
use sdk::utils::parse_contract_input;
use sdk::ContractInput;
use sdk::{erc20::ERC20Action, Digestable, HyleContract, RunResult};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use sha2::{Digest, Sha256};

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod indexer;

pub const TOTAL_SUPPLY: u128 = 100_000_000_000;
pub const FAUCET_ID: &str = "faucet.hydentity";

/// Struct representing the Hyllar token.
#[serde_as]
#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone)]
pub struct Hyllar {
    total_supply: u128,
    balances: BTreeMap<String, u128>, // Balances for each account
    #[serde_as(as = "Vec<(_, _)>")]
    allowances: BTreeMap<(String, String), u128>, // Allowances (owner, spender)
}

impl Default for Hyllar {
    fn default() -> Self {
        Self::custom(FAUCET_ID.to_string())
    }
}

impl Hyllar {
    pub fn custom(faucet_id: String) -> Self {
        let mut balances = BTreeMap::new();
        balances.insert(faucet_id, TOTAL_SUPPLY); // Assign initial supply to faucet
        Hyllar {
            total_supply: TOTAL_SUPPLY,
            balances,
            allowances: BTreeMap::new(),
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }
}

impl HyleContract for Hyllar {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, execution_ctx) = parse_contract_input::<ERC20Action>(contract_input)?;
        let output = self.execute_token_action(action, &execution_ctx);

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, execution_ctx, vec![])),
        }
    }
}

impl ERC20 for Hyllar {
    fn total_supply(&self) -> Result<u128, String> {
        Ok(self.total_supply)
    }

    fn balance_of(&self, account: &str) -> Result<u128, String> {
        match self.balances.get(account) {
            Some(&balance) => Ok(balance),
            None => Err(format!("Account {account} not found")),
        }
    }

    fn transfer(&mut self, sender: &str, recipient: &str, amount: u128) -> Result<(), String> {
        let sender_balance = self.balance_of(sender)?;

        if sender_balance < amount {
            return Err("Insufficient balance".to_string());
        }

        *self.balances.entry(sender.to_string()).or_insert(0) -= amount;
        *self.balances.entry(recipient.to_string()).or_insert(0) += amount;

        Ok(())
    }

    fn transfer_from(
        &mut self,
        owner: &str,
        spender: &str,
        recipient: &str,
        amount: u128,
    ) -> Result<(), String> {
        let allowance = self.allowance(owner, spender)?; // Assuming a fixed spender for simplicity
        let sender_balance = self.balance_of(owner)?;

        if allowance < amount {
            return Err(format!(
                "Allowance exceeded for spender={spender} owner={owner} allowance={allowance}"
            ));
        }
        if sender_balance < amount {
            return Err("Insufficient balance".to_string());
        }

        *self.balances.entry(owner.to_string()).or_insert(0) -= amount;
        *self.balances.entry(recipient.to_string()).or_insert(0) += amount;

        // Decrease the allowance
        let new_allowance = allowance - amount;
        self.allowances
            .insert((owner.to_string(), spender.to_string()), new_allowance);

        Ok(())
    }

    fn approve(&mut self, owner: &str, spender: &str, amount: u128) -> Result<(), String> {
        self.allowances
            .insert((owner.to_string(), spender.to_string()), amount);
        Ok(())
    }

    fn allowance(&self, owner: &str, spender: &str) -> Result<u128, String> {
        match self
            .allowances
            .get(&(owner.to_string(), spender.to_string()))
        {
            Some(&amount) => Ok(amount),
            None => Ok(0), // No allowance set
        }
    }
}

impl Digestable for Hyllar {
    fn as_digest(&self) -> sdk::StateDigest {
        let mut hasher = Sha256::new();
        hasher.update(self.total_supply.to_le_bytes());
        for (account, balance) in self.balances.iter() {
            hasher.update(account.as_bytes());
            hasher.update(balance.to_le_bytes());
        }
        for ((owner, spender), allowance) in self.allowances.iter() {
            hasher.update(owner.as_bytes());
            hasher.update(spender.as_bytes());
            hasher.update(allowance.to_le_bytes());
        }
        sdk::StateDigest(hasher.finalize().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_hyllar_token() {
        let token = Hyllar::default();

        assert_eq!(token.total_supply, TOTAL_SUPPLY);
        assert_eq!(
            token.balances.get(FAUCET_ID).cloned().unwrap_or(0),
            TOTAL_SUPPLY
        );
        assert!(token.allowances.is_empty());
    }

    #[test]
    fn test_balance_of() {
        let initial_supply = TOTAL_SUPPLY;
        let token = Hyllar::default();

        assert_eq!(token.balance_of(FAUCET_ID).unwrap(), initial_supply);
        assert_eq!(
            token.balance_of("nonexistent").unwrap_err(),
            "Account nonexistent not found".to_string()
        );
    }

    #[test]
    fn test_transfer() {
        let mut token = Hyllar::default();

        assert!(token.transfer(FAUCET_ID, "recipient", 500).is_ok());
        assert_eq!(token.balance_of(FAUCET_ID).unwrap(), TOTAL_SUPPLY - 500);
        assert_eq!(token.balance_of("recipient").unwrap(), 500);

        assert!(token
            .transfer(FAUCET_ID, "recipient", TOTAL_SUPPLY)
            .is_err());
    }

    #[test]
    fn test_approve_and_allowance() {
        let mut token = Hyllar::default();

        assert!(token.approve("owner", "spender", 300).is_ok());
        assert_eq!(token.allowance("owner", "spender").unwrap(), 300);
        assert_eq!(token.allowance("owner", "other_spender").unwrap(), 0);
    }

    #[test]
    fn test_transfer_from() {
        let mut token = Hyllar::default();

        assert!(token.approve(FAUCET_ID, "spender", 300).is_ok());

        assert!(token
            .transfer_from(FAUCET_ID, "spender", "recipient", 200)
            .is_ok());
        assert_eq!(token.balance_of(FAUCET_ID).unwrap(), TOTAL_SUPPLY - 200);
        assert_eq!(token.balance_of("recipient").unwrap(), 200);
        assert_eq!(token.allowance(FAUCET_ID, "spender").unwrap(), 100);

        assert_eq!(
            token
                .transfer_from(FAUCET_ID, "spender", "recipient", 200)
                .unwrap_err()
                .to_string(),
            "Allowance exceeded for spender=spender owner=faucet.hydentity allowance=100"
        );
    }

    #[test]
    fn test_transfer_from_unallowed() {
        let mut token = Hyllar::default();

        assert_eq!(
            token
                .transfer_from(FAUCET_ID, "spender", "recipient", 200)
                .unwrap_err()
                .to_string(),
            "Allowance exceeded for spender=spender owner=faucet.hydentity allowance=0"
        );
    }

    #[test]
    fn test_transfer_from_insufficient_balance() {
        let mut token = Hyllar::default();

        // Approve an allowance for the spender
        assert!(token
            .approve(FAUCET_ID, "spender", TOTAL_SUPPLY + 1000)
            .is_ok());

        // Attempt to transfer more than the sender's balance
        let result = token.transfer_from(FAUCET_ID, "spender", "recipient", TOTAL_SUPPLY + 1);

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Insufficient balance".to_string()
        );
    }
}
