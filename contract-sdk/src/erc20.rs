use alloc::{format, string::String};
use bincode::{Decode, Encode};

use crate::{guest::RunResult, HyleContract};

/// Trait representing the ERC-20 token standard interface.
pub trait ERC20
where
    Self: HyleContract,
{
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
}

/// Enum representing possible calls to ERC-20 contract functions.
#[derive(Encode, Decode, Debug, Clone)]
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

/// Executes an action on an object that implements the ERC20 trait based on the ERC20Action enum.
///
/// # Arguments
///
/// * `token` - A mutable reference to an object implementing the ERC20 trait.
/// * `action` - The action to execute, represented as an ERC20Action enum.
pub fn execute_action<T: ERC20>(token: &mut T, action: ERC20Action) -> RunResult {
    let program_outputs;
    let success: bool;
    let identity = token.caller().clone();

    match action {
        ERC20Action::TotalSupply => match token.total_supply() {
            Ok(supply) => {
                success = true;
                program_outputs = format!("Total Supply: {}", supply).into_bytes();
            }
            Err(err) => {
                success = false;
                program_outputs = err.into_bytes();
            }
        },
        ERC20Action::BalanceOf { account } => match token.balance_of(&account) {
            Ok(balance) => {
                success = true;
                program_outputs = format!("Balance of {}: {}", account, balance).into_bytes();
            }
            Err(err) => {
                success = false;
                program_outputs = err.into_bytes();
            }
        },
        ERC20Action::Transfer { recipient, amount } => match token.transfer(&recipient, amount) {
            Ok(_) => {
                success = true;
                program_outputs = format!("Transferred {} to {}", amount, recipient).into_bytes();
            }
            Err(err) => {
                success = false;
                program_outputs = err.into_bytes();
            }
        },
        ERC20Action::TransferFrom {
            sender,
            recipient,
            amount,
        } => match token.transfer_from(&sender, &recipient, amount) {
            Ok(_) => {
                success = true;
                program_outputs =
                    format!("Transferred {} from {} to {}", amount, sender, recipient).into_bytes();
            }
            Err(err) => {
                success = false;
                program_outputs = err.into_bytes();
            }
        },
        ERC20Action::Approve { spender, amount } => match token.approve(&spender, amount) {
            Ok(_) => {
                success = true;
                program_outputs = format!("Approved {} for {}", amount, spender).into_bytes();
            }
            Err(err) => {
                success = false;
                program_outputs = err.into_bytes();
            }
        },
        ERC20Action::Allowance { owner, spender } => match token.allowance(&owner, &spender) {
            Ok(remaining) => {
                success = true;
                program_outputs =
                    format!("Allowance of {} by {}: {}", spender, owner, remaining).into_bytes();
            }
            Err(err) => {
                success = false;
                program_outputs = err.into_bytes();
            }
        },
    }

    RunResult {
        success,
        identity,
        program_outputs,
    }
}

#[cfg(test)]
mod tests {
    use crate::Identity;

    use super::*;
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
        impl HyleContract for ERC20Contract {
            fn caller(&self) -> Identity;
        }
    }

    #[test]
    fn test_total_supply() {
        let mut mock = MockERC20Contract::new();
        mock.expect_total_supply().returning(|| Ok(1000));
        mock.expect_caller().return_const("caller");

        let action = ERC20Action::TotalSupply;
        let result = execute_action(&mut mock, action);

        assert!(result.success);
        assert_eq!(result.program_outputs, b"Total Supply: 1000");
    }

    #[test]
    fn test_balance_of() {
        let mut mock = MockERC20Contract::new();
        mock.expect_balance_of()
            .with(predicate::eq("account1"))
            .returning(|_| Ok(500));
        mock.expect_caller().return_const("caller");

        let action = ERC20Action::BalanceOf {
            account: "account1".to_string(),
        };
        let result = execute_action(&mut mock, action);

        assert!(result.success);
        assert_eq!(result.program_outputs, b"Balance of account1: 500");
    }

    #[test]
    fn test_transfer() {
        let mut mock = MockERC20Contract::new();
        mock.expect_transfer()
            .with(predicate::eq("recipient1"), predicate::eq(200))
            .returning(|_, _| Ok(()));
        mock.expect_caller().return_const("caller");

        let action = ERC20Action::Transfer {
            recipient: "recipient1".to_string(),
            amount: 200,
        };
        let result = execute_action(&mut mock, action);

        assert!(result.success);
        assert_eq!(result.program_outputs, b"Transferred 200 to recipient1");
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
        mock.expect_caller().return_const("caller");

        let action = ERC20Action::TransferFrom {
            sender: "sender1".to_string(),
            recipient: "recipient1".to_string(),
            amount: 300,
        };
        let result = execute_action(&mut mock, action);

        assert!(result.success);
        assert_eq!(
            result.program_outputs,
            b"Transferred 300 from sender1 to recipient1"
        );
    }

    #[test]
    fn test_approve() {
        let mut mock = MockERC20Contract::new();
        mock.expect_approve()
            .with(predicate::eq("spender1"), predicate::eq(400))
            .returning(|_, _| Ok(()));
        mock.expect_caller().return_const("caller");

        let action = ERC20Action::Approve {
            spender: "spender1".to_string(),
            amount: 400,
        };
        let result = execute_action(&mut mock, action);

        assert!(result.success);
        assert_eq!(result.program_outputs, b"Approved 400 for spender1");
    }

    #[test]
    fn test_allowance() {
        let mut mock = MockERC20Contract::new();
        mock.expect_allowance()
            .with(predicate::eq("owner1"), predicate::eq("spender1"))
            .returning(|_, _| Ok(500));
        mock.expect_caller().return_const("caller");

        let action = ERC20Action::Allowance {
            owner: "owner1".to_string(),
            spender: "spender1".to_string(),
        };
        let result = execute_action(&mut mock, action);

        assert!(result.success);
        assert_eq!(
            result.program_outputs,
            b"Allowance of spender1 by owner1: 500"
        );
    }
}
