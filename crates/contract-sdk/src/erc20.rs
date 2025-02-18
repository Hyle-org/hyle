use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use hyle_model::{Blob, BlobData, BlobIndex, ContractAction, ContractName, StructuredBlobData};

use crate::caller::{CallerCallee, CheckCalleeBlobs};

/// Trait representing the ERC-20 token standard interface.
pub trait ERC20 {
    /// Returns the total supply of tokens in existence.
    ///
    /// # Returns
    ///
    /// * `Result<u128, String>` - The total supply of tokens on success, or an error message on failure.
    fn total_supply(&self) -> Result<u128, String>;

    /// Returns the balance of tokens for a given account.
    ///
    /// # Arguments
    ///
    /// * `account` - The address of the account as a string slice.
    ///
    /// # Returns
    ///
    /// * `Result<u128, String>` - The token balance of the specified account on success, or an error message on failure.
    fn balance_of(&self, account: &str) -> Result<u128, String>;

    /// Transfers a specified amount of tokens to a recipient address.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The address of the recipient as a string slice.
    /// * `amount` - The amount of tokens to transfer.
    ///
    /// # Returns
    ///
    /// * `Result<(), String>` - `Ok(())` if the transfer was successful, or an error message on failure.
    fn transfer(&mut self, recipient: &str, amount: u128) -> Result<(), String>;

    /// Transfers tokens from a sender address to a recipient address using a pre-approved allowance.
    ///
    /// # Arguments
    ///
    /// * `sender` - The address of the token holder as a string slice.
    /// * `recipient` - The address of the recipient as a string slice.
    /// * `amount` - The amount of tokens to transfer.
    ///
    /// # Returns
    ///
    /// * `Result<(), String>` - `Ok(())` if the transfer was successful, or an error message on failure.
    fn transfer_from(&mut self, sender: &str, recipient: &str, amount: u128) -> Result<(), String>;

    /// Approves an address to spend a specified amount of tokens on behalf of the caller.
    ///
    /// # Arguments
    ///
    /// * `spender` - The address authorized to spend the tokens as a string slice.
    /// * `amount` - The maximum amount of tokens that the spender is authorized to spend.
    ///
    /// # Returns
    ///
    /// * `Result<(), String>` - `Ok(())` if the approval was successful, or an error message on failure.
    fn approve(&mut self, spender: &str, amount: u128) -> Result<(), String>;

    /// Returns the remaining amount of tokens that `spender` is allowed to spend on behalf of `owner`.
    ///
    /// # Arguments
    ///
    /// * `owner` - The address of the token owner as a string slice.
    /// * `spender` - The address authorized to spend the tokens as a string slice.
    ///
    /// # Returns
    ///
    /// * `Result<u128, String>` - The remaining allowance on success, or an error message on failure.
    fn allowance(&self, owner: &str, spender: &str) -> Result<u128, String>;

    /// Executes an action on an object that implements the ERC20 trait based on the ERC20Action enum.
    ///
    /// # Arguments
    ///
    /// * `action` - The action to execute, represented as an ERC20Action enum.
    /// * `private_input` - A string representing the private input for the action.
    fn execute_token_action(&mut self, action: ERC20Action) -> Result<String, String> {
        match action {
            ERC20Action::TotalSupply => self
                .total_supply()
                .map(|supply| format!("Total Supply: {}", supply)),
            ERC20Action::BalanceOf { account } => self
                .balance_of(&account)
                .map(|balance| format!("Balance of {}: {}", account, balance)),
            ERC20Action::Transfer { recipient, amount } => self
                .transfer(&recipient, amount)
                .map(|_| format!("Transferred {} to {}", amount, recipient)),
            ERC20Action::TransferFrom {
                sender,
                recipient,
                amount,
            } => self
                .transfer_from(&sender, &recipient, amount)
                .map(|_| format!("Transferred {} from {} to {}", amount, sender, recipient)),
            ERC20Action::Approve { spender, amount } => self
                .approve(&spender, amount)
                .map(|_| format!("Approved {} for {}", amount, spender,)),
            ERC20Action::Allowance { owner, spender } => self
                .allowance(&owner, &spender)
                .map(|allowance| format!("Allowance of {} by {}: {}", spender, owner, allowance)),
        }
    }
}

/// Enum representing possible calls to ERC-20 contract functions.
#[derive(Serialize, Deserialize, BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq)]
pub enum ERC20Action {
    TotalSupply,
    BalanceOf {
        account: String,
    },
    Transfer {
        recipient: String,
        amount: u128,
    },
    TransferFrom {
        sender: String,
        recipient: String,
        amount: u128,
    },
    Approve {
        spender: String,
        amount: u128,
    },
    Allowance {
        owner: String,
        spender: String,
    },
}

impl ContractAction for ERC20Action {
    fn as_blob(
        &self,
        contract_name: ContractName,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        Blob {
            contract_name,
            data: BlobData::from(StructuredBlobData {
                caller,
                callees,
                parameters: self.clone(),
            }),
        }
    }
}

pub struct ERC20BlobChecker<'a, T>(&'a ContractName, &'a T);

impl<'a, T> ERC20BlobChecker<'a, T> {
    pub fn new(contract_name: &'a ContractName, contract: &'a T) -> Self {
        Self(contract_name, contract)
    }
}

// Had to implement this for &mut T or it can't be found when used in &mut self methods.
impl<T> ERC20 for ERC20BlobChecker<'_, &mut T>
where
    T: CallerCallee + CheckCalleeBlobs,
{
    fn total_supply(&self) -> Result<u128, String> {
        unimplemented!()
    }

    fn balance_of(&self, _account: &str) -> Result<u128, String> {
        unimplemented!()
    }

    fn transfer(&mut self, recipient: &str, amount: u128) -> Result<(), String> {
        self.1.is_in_callee_blobs(
            self.0,
            ERC20Action::Transfer {
                recipient: recipient.to_string(),
                amount,
            },
        )
    }

    fn transfer_from(&mut self, sender: &str, recipient: &str, amount: u128) -> Result<(), String> {
        self.1.is_in_callee_blobs(
            self.0,
            ERC20Action::TransferFrom {
                sender: sender.to_string(),
                recipient: recipient.to_string(),
                amount,
            },
        )
    }
    fn approve(&mut self, _spender: &str, _amount: u128) -> Result<(), String> {
        unimplemented!()
    }
    fn allowance(&self, _owner: &str, _spender: &str) -> Result<u128, String> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use hyle_model::Digestable;
    use mockall::{
        mock,
        predicate::{self, *},
    };
    extern crate std;

    mock! {
        pub ERC20Contract {}
        impl ERC20 for ERC20Contract {
            fn total_supply(&self) -> Result<u128, String>;
            fn balance_of(&self, account: &str) -> Result<u128, String>;
            fn transfer(&mut self, recipient: &str, amount: u128) -> Result<(), String>;
            fn transfer_from(&mut self, sender: &str, recipient: &str, amount: u128) -> Result<(), String>;
            fn approve(&mut self, spender: &str, amount: u128) -> Result<(), String>;
            fn allowance(&self, owner: &str, spender: &str) -> Result<u128, String>;
        }
        impl Digestable for ERC20Contract {
            fn as_digest(&self) -> hyle_model::StateDigest {
                hyle_model::sdk::StateDigest(vec![])
            }
        }
    }

    #[test]
    fn test_total_supply() {
        let mut mock = MockERC20Contract::new();
        mock.expect_total_supply().returning(|| Ok(1000));

        let action = ERC20Action::TotalSupply;
        let result = mock.execute_token_action(action);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Total Supply: 1000");
    }

    #[test]
    fn test_balance_of() {
        let mut mock = MockERC20Contract::new();
        mock.expect_balance_of()
            .with(predicate::eq("account1"))
            .returning(|_| Ok(500));

        let action = ERC20Action::BalanceOf {
            account: "account1".to_string(),
        };
        let result = mock.execute_token_action(action);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Balance of account1: 500");
    }

    #[test]
    fn test_transfer() {
        let mut mock = MockERC20Contract::new();
        mock.expect_transfer()
            .with(predicate::eq("recipient1"), predicate::eq(200))
            .returning(|_, _| Ok(()));

        let action = ERC20Action::Transfer {
            recipient: "recipient1".to_string(),
            amount: 200,
        };
        let result = mock.execute_token_action(action);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Transferred 200 to recipient1");
    }

    #[test]
    fn test_transfer_from() {
        let mut mock = MockERC20Contract::new();
        mock.expect_transfer_from()
            .with(
                predicate::eq("sender1"),
                predicate::eq("recipient1"),
                predicate::eq(300),
            )
            .returning(|_, _, _| Ok(()));

        let action = ERC20Action::TransferFrom {
            sender: "sender1".to_string(),
            recipient: "recipient1".to_string(),
            amount: 300,
        };
        let result = mock.execute_token_action(action);

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "Transferred 300 from sender1 to recipient1"
        );
    }

    #[test]
    fn test_approve() {
        let mut mock = MockERC20Contract::new();
        mock.expect_approve()
            .with(predicate::eq("spender1"), predicate::eq(400))
            .returning(|_, _| Ok(()));

        let action = ERC20Action::Approve {
            spender: "spender1".to_string(),
            amount: 400,
        };
        let result = mock.execute_token_action(action);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Approved 400 for spender1");
    }

    #[test]
    fn test_allowance() {
        let mut mock = MockERC20Contract::new();
        mock.expect_allowance()
            .with(predicate::eq("owner1"), predicate::eq("spender1"))
            .returning(|_, _| Ok(500));

        let action = ERC20Action::Allowance {
            owner: "owner1".to_string(),
            spender: "spender1".to_string(),
        };
        let result = mock.execute_token_action(action);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Allowance of spender1 by owner1: 500");
    }
}
