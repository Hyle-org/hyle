use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sdk::erc20::ERC20;
use sdk::utils::parse_contract_input_with_context;
use sdk::ContractInput;
use sdk::{erc20::ERC20Action, Digestable, HyleContract, RunResult};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod indexer;

/// Struct representing the Hyllar token.
#[serde_as]
#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone)]
pub struct Hyllar {
    total_supply: u128,
    balances: BTreeMap<String, u128>, // Balances for each account
    #[serde_as(as = "Vec<(_, _)>")]
    allowances: BTreeMap<(String, String), u128>, // Allowances (owner, spender)
}

impl Hyllar {
    /// Creates a new Hyllar token with the specified initial supply.
    ///
    /// # Arguments
    ///
    /// * `initial_supply` - The initial supply of the token.
    ///
    /// # Returns
    ///
    /// * `HyllarToken` - A new instance of the Hyllar token.
    pub fn new(initial_supply: u128, faucet_id: String) -> Self {
        let mut balances = BTreeMap::new();
        balances.insert(faucet_id, initial_supply); // Assign initial supply to faucet
        Hyllar {
            total_supply: initial_supply,
            balances,
            allowances: BTreeMap::new(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }
}

impl HyleContract for Hyllar {
    fn execute_action(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, execution_ctx) =
            parse_contract_input_with_context::<ERC20Action>(contract_input)?;
        let output = self.execute_token_action(action, execution_ctx);

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, vec![])),
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
        sdk::StateDigest(self.to_bytes())
    }
}

impl TryFrom<sdk::StateDigest> for Hyllar {
    type Error = anyhow::Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        borsh::from_slice(&state.0).map_err(|_| anyhow::anyhow!("Could not decode hyllar state"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_hyllar_token() {
        let initial_supply = 1000;
        let token = Hyllar::new(initial_supply, "faucet".to_string());

        assert_eq!(token.total_supply, initial_supply);
        assert_eq!(
            token.balances.get("faucet").cloned().unwrap_or(0),
            initial_supply
        );
        assert!(token.allowances.is_empty());
    }

    #[test]
    fn test_total_supply() {
        let initial_supply = 1000;
        let token = Hyllar::new(initial_supply, "faucet".to_string());

        assert_eq!(token.total_supply().unwrap(), initial_supply);
    }

    #[test]
    fn test_balance_of() {
        let initial_supply = 1000;
        let token = Hyllar::new(initial_supply, "faucet".to_string());

        assert_eq!(token.balance_of("faucet").unwrap(), initial_supply);
        assert_eq!(
            token.balance_of("nonexistent").unwrap_err(),
            "Account nonexistent not found".to_string()
        );
    }

    #[test]
    fn test_transfer() {
        let initial_supply = 1000;
        let mut token = Hyllar::new(initial_supply, "faucet".to_string());

        assert!(token.transfer("faucet", "recipient", 500).is_ok());
        assert_eq!(token.balance_of("faucet").unwrap(), 500);
        assert_eq!(token.balance_of("recipient").unwrap(), 500);

        assert!(token.transfer("faucet", "recipient", 600).is_err());
    }

    #[test]
    fn test_approve_and_allowance() {
        let initial_supply = 1000;
        let mut token = Hyllar::new(initial_supply, "faucet".to_string());

        assert!(token.approve("owner", "spender", 300).is_ok());
        assert_eq!(token.allowance("owner", "spender").unwrap(), 300);
        assert_eq!(token.allowance("owner", "other_spender").unwrap(), 0);
    }

    #[test]
    fn test_transfer_from() {
        let initial_supply = 1000;
        let mut token = Hyllar::new(initial_supply, "faucet".to_string());

        assert!(token.approve("owner", "faucet", 300).is_ok());

        assert!(token
            .transfer_from("owner", "faucet", "recipient", 200)
            .is_ok());
        assert_eq!(token.balance_of("faucet").unwrap(), 800);
        assert_eq!(token.balance_of("recipient").unwrap(), 200);
        assert_eq!(token.allowance("faucet", "spender").unwrap(), 100);

        assert_eq!(
            token
                .transfer_from("owner", "faucet", "recipient", 200)
                .unwrap_err()
                .to_string(),
            "Allowance exceeded for spender=faucet owner=spender allowance=100"
        );
    }

    #[test]
    fn test_transfer_from_unallowed() {
        let initial_supply = 1000;
        let mut token = Hyllar::new(initial_supply, "faucet".to_string());

        assert_eq!(
            token
                .transfer_from("faucet", "spender", "recipient", 200)
                .unwrap_err()
                .to_string(),
            "Allowance exceeded for spender=spender owner=faucet allowance=0"
        );
    }

    #[test]
    fn test_transfer_from_insufficient_balance() {
        let initial_supply = 1000;
        let mut token = Hyllar::new(initial_supply, "faucet".to_string());

        // Approve an allowance for the spender
        assert!(token.approve("faucet", "spender", 5000).is_ok());

        // Attempt to transfer more than the sender's balance
        let result = token.transfer_from("faucet", "spender", "recipient", 1100);

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Insufficient balance".to_string()
        );
    }

    #[test]
    fn test_as_digest() {
        let initial_supply = 1000;
        let token = Hyllar::new(initial_supply, "faucet".to_string());
        let digest = token.as_digest();

        let encoded = borsh::to_vec(&token).expect("Failed to encode Balances");
        assert_eq!(digest.0, encoded);
    }

    #[test]
    fn test_try_from_state_digest() {
        let initial_supply = 1000;
        let token = Hyllar::new(initial_supply, "faucet".to_string());
        let digest = token.as_digest();

        let decoded_token: Hyllar =
            Hyllar::try_from(digest.clone()).expect("Failed to decode state digest");
        assert_eq!(decoded_token.total_supply, token.total_supply);
        assert_eq!(decoded_token.balances, token.balances);

        let invalid_digest = sdk::StateDigest(vec![0, 1, 2, 3]);
        let result = Hyllar::try_from(invalid_digest);
        assert!(result.is_err());
    }
}
