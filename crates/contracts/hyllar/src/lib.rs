use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sdk::erc20::ERC20;
use sdk::ContractInput;
use sdk::{
    caller::{CalleeBlobs, CallerCallee, ExecutionContext, MutCalleeBlobs},
    erc20::ERC20Action,
    HyleContract, Digestable, Identity, RunResult,
};
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
pub struct HyllarState {
    total_supply: u128,
    balances: BTreeMap<String, u128>, // Balances for each account
    #[serde_as(as = "Vec<(_, _)>")]
    allowances: BTreeMap<(String, String), u128>, // Allowances (owner, spender)
}

#[derive(Debug)]
pub struct HyllarContract {
    exec_ctx: ExecutionContext,
    state: HyllarState,
}

impl HyllarState {
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
        HyllarState {
            total_supply: initial_supply,
            balances,
            allowances: BTreeMap::new(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).expect("Failed to encode Balances")
    }
}

impl HyleContract<HyllarState, ERC20Action> for HyllarContract {
    fn execute_action(&mut self, action: ERC20Action, _: &ContractInput) -> RunResult<HyllarState>
    where
        Self: Sized,
    {
        let output = self.execute_token_action(action);

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, self.state.clone(), vec![])),
        }
    }

    fn init(state: HyllarState, exec_ctx: ExecutionContext) -> Self {
        HyllarContract { state, exec_ctx }
    }

    fn state(self) -> HyllarState {
        self.state
    }
}

impl CallerCallee for HyllarContract {
    fn caller(&self) -> &Identity {
        &self.exec_ctx.caller
    }
    fn callee_blobs(&self) -> CalleeBlobs {
        CalleeBlobs(self.exec_ctx.callees_blobs.borrow())
    }
    fn mut_callee_blobs(&self) -> MutCalleeBlobs {
        MutCalleeBlobs(self.exec_ctx.callees_blobs.borrow_mut())
    }
}

impl ERC20 for HyllarContract {
    fn total_supply(&self) -> Result<u128, String> {
        Ok(self.state.total_supply)
    }

    fn balance_of(&self, account: &str) -> Result<u128, String> {
        match self.state.balances.get(account) {
            Some(&balance) => Ok(balance),
            None => Err(format!("Account {account} not found")),
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
        let caller = self.caller().clone();
        let allowance = self.allowance(sender, caller.0.as_str())?; // Assuming a fixed spender for simplicity
        let sender_balance = self.balance_of(sender)?;

        if allowance < amount {
            return Err(format!(
                "Allowance exceeded for sender={sender} caller={caller} allowance={allowance}"
            ));
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
        let owner = self.caller().clone().0;
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

impl Digestable for HyllarState {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(self.to_bytes())
    }
}
impl Digestable for HyllarContract {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(self.state.to_bytes())
    }
}

impl TryFrom<sdk::StateDigest> for HyllarState {
    type Error = anyhow::Error;

    fn try_from(state: sdk::StateDigest) -> Result<Self, Self::Error> {
        borsh::from_slice(&state.0).map_err(|_| anyhow::anyhow!("Could not decode hyllar state"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdk::Identity;

    #[test]
    fn test_new_hyllar_token() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());

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
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let exec_ctx = ExecutionContext {
            caller: Identity::new("caller"),
            ..ExecutionContext::default()
        };
        let contract = HyllarContract::init(token, exec_ctx);

        assert_eq!(contract.total_supply().unwrap(), initial_supply);
    }

    #[test]
    fn test_balance_of() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let exec_ctx = ExecutionContext {
            caller: Identity::new("caller"),
            ..ExecutionContext::default()
        };
        let contract = HyllarContract::init(token, exec_ctx);

        assert_eq!(contract.balance_of("faucet").unwrap(), initial_supply);
        assert_eq!(
            contract.balance_of("nonexistent").unwrap_err(),
            "Account nonexistent not found".to_string()
        );
    }

    #[test]
    fn test_transfer() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let exec_ctx = ExecutionContext {
            caller: Identity::new("faucet"),
            ..ExecutionContext::default()
        };
        let mut contract = HyllarContract::init(token, exec_ctx);

        assert!(contract.transfer("recipient", 500).is_ok());
        assert_eq!(contract.balance_of("faucet").unwrap(), 500);
        assert_eq!(contract.balance_of("recipient").unwrap(), 500);

        assert!(contract.transfer("recipient", 600).is_err());
    }

    #[test]
    fn test_approve_and_allowance() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let exec_ctx = ExecutionContext {
            caller: Identity::new("owner"),
            ..ExecutionContext::default()
        };
        let mut contract = HyllarContract::init(token, exec_ctx);

        assert!(contract.approve("spender", 300).is_ok());
        assert_eq!(contract.allowance("owner", "spender").unwrap(), 300);
        assert_eq!(contract.allowance("owner", "other_spender").unwrap(), 0);
    }

    #[test]
    fn test_transfer_from() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let exec_ctx = ExecutionContext {
            caller: Identity::new("faucet"),
            ..ExecutionContext::default()
        };
        let mut contract = HyllarContract::init(token, exec_ctx);

        assert!(contract.approve("spender", 300).is_ok());
        let exec_ctx = ExecutionContext {
            caller: Identity::new("spender"),
            ..ExecutionContext::default()
        };
        let mut contract = HyllarContract::init(contract.state(), exec_ctx);

        assert!(contract.transfer_from("faucet", "recipient", 200).is_ok());
        assert_eq!(contract.balance_of("faucet").unwrap(), 800);
        assert_eq!(contract.balance_of("recipient").unwrap(), 200);
        assert_eq!(contract.allowance("faucet", "spender").unwrap(), 100);

        assert_eq!(
            contract
                .transfer_from("faucet", "recipient", 200)
                .unwrap_err()
                .to_string(),
            "Allowance exceeded for sender=faucet caller=spender allowance=100"
        );
    }

    #[test]
    fn test_transfer_from_unallowed() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let exec_ctx = ExecutionContext {
            caller: Identity::new("spender"),
            ..ExecutionContext::default()
        };
        let mut contract = HyllarContract::init(token, exec_ctx);

        assert_eq!(
            contract
                .transfer_from("faucet", "recipient", 200)
                .unwrap_err()
                .to_string(),
            "Allowance exceeded for sender=faucet caller=spender allowance=0"
        );
    }

    #[test]
    fn test_transfer_from_insufficient_balance() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let exec_ctx = ExecutionContext {
            caller: Identity::new("faucet"),
            ..ExecutionContext::default()
        };
        let mut contract = HyllarContract::init(token, exec_ctx);

        // Approve an allowance for the spender
        assert!(contract.approve("spender", 5000).is_ok());

        // Attempt to transfer more than the sender's balance
        let exec_ctx = ExecutionContext {
            caller: Identity::new("spender"),
            ..ExecutionContext::default()
        };
        let mut contract = HyllarContract::init(contract.state(), exec_ctx);
        let result = contract.transfer_from("faucet", "recipient", 1100);

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Insufficient balance".to_string()
        );
    }

    #[test]
    fn test_as_digest() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let digest = token.as_digest();

        let encoded = borsh::to_vec(&token).expect("Failed to encode Balances");
        assert_eq!(digest.0, encoded);
    }

    #[test]
    fn test_try_from_state_digest() {
        let initial_supply = 1000;
        let token = HyllarState::new(initial_supply, "faucet".to_string());
        let digest = token.as_digest();

        let decoded_token: HyllarState =
            HyllarState::try_from(digest.clone()).expect("Failed to decode state digest");
        assert_eq!(decoded_token.total_supply, token.total_supply);
        assert_eq!(decoded_token.balances, token.balances);

        let invalid_digest = sdk::StateDigest(vec![0, 1, 2, 3]);
        let result = HyllarState::try_from(invalid_digest);
        assert!(result.is_err());
    }
}
