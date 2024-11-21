use std::collections::HashMap;

use anyhow::Error;
use bincode::{Decode, Encode};
use sdk::{erc20::ERC20, Digestable, HyleContract, Identity};
use serde::{Deserialize, Serialize};

/// Struct representing the Hyllar token.
#[derive(Encode, Decode, Serialize, Deserialize, Debug, Clone)]
pub struct HyllarToken {
    total_supply: u128,
    balances: HashMap<String, u128>, // Balances for each account
    allowances: HashMap<(String, String), u128>, // Allowances (owner, spender)
}

pub struct HyllarTokenContract {
    state: HyllarToken,
    caller: Identity,
}

impl HyllarToken {
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
        let mut balances = HashMap::new();
        balances.insert(faucet_id, initial_supply); // Assign initial supply to faucet
        HyllarToken {
            total_supply: initial_supply,
            balances,
            allowances: HashMap::new(),
        }
    }
}

impl HyllarTokenContract {
    pub fn init(state: HyllarToken, caller: Identity) -> HyllarTokenContract {
        HyllarTokenContract { state, caller }
    }
    pub fn state(self) -> HyllarToken {
        self.state
    }
}

impl HyleContract for HyllarTokenContract {
    fn caller(&self) -> Identity {
        self.caller.clone()
    }
}

impl ERC20 for HyllarTokenContract {
    fn total_supply(&self) -> Result<u128, String> {
        Ok(self.state.total_supply)
    }

    fn balance_of(&self, account: &str) -> Result<u128, String> {
        match self.state.balances.get(account) {
            Some(&balance) => Ok(balance),
            None => Err("Account not found".to_string()),
        }
    }

    fn transfer(&mut self, recipient: &str, amount: u128) -> Result<(), String> {
        let sender = self.caller();
        let sender = sender.0.as_str();
        let sender_balance = self.balance_of(sender)?;

        if sender_balance < amount {
            return Err("Insufficient balance".to_string());
        }

        *self.state.balances.entry(sender.to_string()).or_insert(0) -= amount;
        *self
            .state
            .balances
            .entry(recipient.to_string())
            .or_insert(0) += amount;

        Ok(())
    }

    fn transfer_from(&mut self, sender: &str, recipient: &str, amount: u128) -> Result<(), String> {
        let caller = self.caller();
        let allowance = self.allowance(sender, caller.0.as_str())?; // Assuming a fixed spender for simplicity
        let sender_balance = self.balance_of(sender)?;

        if allowance < amount {
            return Err("Allowance exceeded".to_string());
        }
        if sender_balance < amount {
            return Err("Insufficient balance".to_string());
        }

        *self.state.balances.entry(sender.to_string()).or_insert(0) -= amount;
        *self
            .state
            .balances
            .entry(recipient.to_string())
            .or_insert(0) += amount;

        // Decrease the allowance
        let new_allowance = allowance - amount;
        self.state
            .allowances
            .insert((sender.to_string(), caller.0), new_allowance);

        Ok(())
    }

    fn approve(&mut self, spender: &str, amount: u128) -> Result<(), String> {
        let owner = self.caller().0;
        self.state
            .allowances
            .insert((owner, spender.to_string()), amount);
        Ok(())
    }

    fn allowance(&self, owner: &str, spender: &str) -> Result<u128, String> {
        match self
            .state
            .allowances
            .get(&(owner.to_string(), spender.to_string()))
        {
            Some(&amount) => Ok(amount),
            None => Ok(0), // No allowance set
        }
    }
}

impl Digestable for HyllarToken {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(
            bincode::encode_to_vec(self, bincode::config::standard())
                .expect("Failed to encode Balances"),
        )
    }
}
impl TryFrom<sdk::StateDigest> for HyllarToken {
    type Error = Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        let (balances, _) = bincode::decode_from_slice(&state.0, bincode::config::standard())
            .map_err(|_| anyhow::anyhow!("Could not decode start height"))?;
        Ok(balances)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdk::Identity;

    #[test]
    fn test_new_hyllar_token() {
        let initial_supply = 1000;
        let token = HyllarToken::new(initial_supply, "faucet".to_string());

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
        let token = HyllarToken::new(initial_supply, "faucet".to_string());
        let contract = HyllarTokenContract::init(token, Identity("caller".to_string()));

        assert_eq!(contract.total_supply().unwrap(), initial_supply);
    }

    #[test]
    fn test_balance_of() {
        let initial_supply = 1000;
        let token = HyllarToken::new(initial_supply, "faucet".to_string());
        let contract = HyllarTokenContract::init(token, Identity("caller".to_string()));

        assert_eq!(contract.balance_of("faucet").unwrap(), initial_supply);
        assert_eq!(
            contract.balance_of("nonexistent").unwrap_err(),
            "Account not found".to_string()
        );
    }

    #[test]
    fn test_transfer() {
        let initial_supply = 1000;
        let token = HyllarToken::new(initial_supply, "faucet".to_string());
        let mut contract = HyllarTokenContract::init(token, Identity("faucet".to_string()));

        assert!(contract.transfer("recipient", 500).is_ok());
        assert_eq!(contract.balance_of("faucet").unwrap(), 500);
        assert_eq!(contract.balance_of("recipient").unwrap(), 500);

        assert!(contract.transfer("recipient", 600).is_err());
    }

    #[test]
    fn test_approve_and_allowance() {
        let initial_supply = 1000;
        let token = HyllarToken::new(initial_supply, "faucet".to_string());
        let mut contract = HyllarTokenContract::init(token, Identity("owner".to_string()));

        assert!(contract.approve("spender", 300).is_ok());
        assert_eq!(contract.allowance("owner", "spender").unwrap(), 300);
        assert_eq!(contract.allowance("owner", "other_spender").unwrap(), 0);
    }

    #[test]
    fn test_transfer_from() {
        let initial_supply = 1000;
        let token = HyllarToken::new(initial_supply, "faucet".to_string());
        let mut contract = HyllarTokenContract::init(token, Identity("faucet".to_string()));

        assert!(contract.approve("spender", 300).is_ok());
        let mut contract =
            HyllarTokenContract::init(contract.state(), Identity("spender".to_string()));

        assert!(contract.transfer_from("faucet", "recipient", 200).is_ok());
        assert_eq!(contract.balance_of("faucet").unwrap(), 800);
        assert_eq!(contract.balance_of("recipient").unwrap(), 200);
        assert_eq!(contract.allowance("faucet", "spender").unwrap(), 100);

        assert!(contract.transfer_from("faucet", "recipient", 200).is_err());
    }

    #[test]
    fn test_transfer_from_insufficient_balance() {
        let initial_supply = 1000;
        let token = HyllarToken::new(initial_supply, "faucet".to_string());
        let mut contract = HyllarTokenContract::init(token, Identity("faucet".to_string()));

        // Approve an allowance for the spender
        assert!(contract.approve("spender", 5000).is_ok());

        // Attempt to transfer more than the sender's balance
        let mut contract =
            HyllarTokenContract::init(contract.state(), Identity("spender".to_string()));
        let result = contract.transfer_from("faucet", "recipient", 1100);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Insufficient balance".to_string());
    }

    #[test]
    fn test_as_digest() {
        let initial_supply = 1000;
        let token = HyllarToken::new(initial_supply, "faucet".to_string());
        let digest = token.as_digest();

        let encoded = bincode::encode_to_vec(&token, bincode::config::standard())
            .expect("Failed to encode Balances");
        assert_eq!(digest.0, encoded);
    }

    #[test]
    fn test_try_from_state_digest() {
        let initial_supply = 1000;
        let token = HyllarToken::new(initial_supply, "faucet".to_string());
        let digest = token.as_digest();

        let decoded_token: HyllarToken =
            HyllarToken::try_from(digest.clone()).expect("Failed to decode state digest");
        assert_eq!(decoded_token.total_supply, token.total_supply);
        assert_eq!(decoded_token.balances, token.balances);

        let invalid_digest = sdk::StateDigest(vec![0, 1, 2, 3]);
        let result = HyllarToken::try_from(invalid_digest);
        assert!(result.is_err());
    }
}
