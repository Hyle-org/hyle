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
