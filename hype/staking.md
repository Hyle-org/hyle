# Staking contract 

First we need an ERC-20 token "hyllar" 

## ERC20 Hyllar 

```rust 
/// Trait representing the ERC-20 token standard interface.
pub trait ERC20 {
    /// Returns the total supply of tokens in existence.
    ///
    /// # Returns
    ///
    /// * `u128` - The total supply of tokens.
    fn total_supply(&self) -> u128;

    /// Returns the balance of tokens for a given account.
    ///
    /// # Arguments
    ///
    /// * `account` - The address of the account as a string slice.
    ///
    /// # Returns
    ///
    /// * `u128` - The token balance of the specified account.
    fn balance_of(&self, account: &str) -> u128;

    /// Transfers a specified amount of tokens to a recipient address.
    ///
    /// # Arguments
    ///
    /// * `recipient` - The address of the recipient as a string slice.
    /// * `amount` - The amount of tokens to transfer.
    ///
    /// # Returns
    ///
    /// * `bool` - `true` if the transfer was successful, otherwise `false`.
    fn transfer(&mut self, recipient: &str, amount: u128) -> bool;

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
    /// * `bool` - `true` if the transfer was successful, otherwise `false`.
    fn transfer_from(&mut self, sender: &str, recipient: &str, amount: u128) -> bool;

    /// Approves an address to spend a specified amount of tokens on behalf of the caller.
    ///
    /// # Arguments
    ///
    /// * `spender` - The address authorized to spend the tokens as a string slice.
    /// * `amount` - The maximum amount of tokens that the spender is authorized to spend.
    ///
    /// # Returns
    ///
    /// * `bool` - `true` if the approval was successful, otherwise `false`.
    fn approve(&mut self, spender: &str, amount: u128) -> bool;

    /// Returns the remaining amount of tokens that `spender` is allowed to spend on behalf of `owner`.
    ///
    /// # Arguments
    ///
    /// * `owner` - The address of the token owner as a string slice.
    /// * `spender` - The address authorized to spend the tokens as a string slice.
    ///
    /// # Returns
    ///
    /// * `u128` - The remaining allowance.
    fn allowance(&self, owner: &str, spender: &str) -> u128;

    /// Emits a transfer event. In a real blockchain implementation, this would be an event log.
    ///
    /// # Arguments
    ///
    /// * `from` - The address of the sender as a string slice.
    /// * `to` - The address of the recipient as a string slice.
    /// * `value` - The amount of tokens transferred.
    fn emit_transfer(&self, from: &str, to: &str, value: u128);

    /// Emits an approval event. In a real blockchain implementation, this would be an event log.
    ///
    /// # Arguments
    ///
    /// * `owner` - The address of the token owner as a string slice.
    /// * `spender` - The address authorized to spend the tokens as a string slice.
    /// * `value` - The amount of tokens approved for the spender.
    fn emit_approval(&self, owner: &str, spender: &str, value: u128);
}

    
```
