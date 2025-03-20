use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use sdk::{utils::parse_contract_input, ContractInput, HyleContract, RunResult};

use crate::{
    account::{Account, AccountSMT},
    SmtTokenAction,
};

/// This struct is necessary to allow the indexer to access the full state
/// (including all accounts, their balances, and allowances) as the other
/// implementation only uses the commitment.
#[derive(Default, Debug)]
pub struct SmtTokenState {
    pub accounts: AccountSMT,
}

impl SmtTokenState {
    pub fn new(accounts: AccountSMT) -> Self {
        Self { accounts }
    }
}

impl HyleContract for SmtTokenState {
    /// !!! WARNINGS !!!
    /// This function is only here to keep track of the balances.
    /// No checks are done to verify that this is a legit action.
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, execution_ctx) = parse_contract_input::<SmtTokenAction>(contract_input)?;
        let output = match action {
            SmtTokenAction::Transfer {
                proof: _,
                mut sender_account,
                mut recipient_account,
                amount,
            } => {
                let sender_key = sender_account.get_key();
                let recipient_key = recipient_account.get_key();

                sender_account.balance -= amount;
                recipient_account.balance += amount;

                if let Err(e) = self.accounts.update(sender_key, sender_account) {
                    return Err(format!("Failed to update sender account: {e}"));
                }
                if let Err(e) = self
                    .accounts
                    .update(recipient_key, recipient_account.clone())
                {
                    return Err(format!("Failed to update recipient account: {e}"));
                }
                Ok(format!(
                    "Transferred {} to {}",
                    amount, recipient_account.address
                ))
            }
            SmtTokenAction::TransferFrom {
                proof: _,
                mut owner_account,
                spender: _,
                mut recipient_account,
                amount,
            } => {
                let owner_key = owner_account.get_key();
                let recipient_key = recipient_account.get_key();

                owner_account.balance -= amount;
                recipient_account.balance += amount;
                if let Err(e) = self.accounts.update(owner_key, owner_account) {
                    return Err(format!("Failed to update owner account: {e}"));
                }
                if let Err(e) = self
                    .accounts
                    .update(recipient_key, recipient_account.clone())
                {
                    return Err(format!("Failed to update recipient account: {e}"));
                }
                Ok(format!(
                    "Transferred {} to {}",
                    amount, recipient_account.address
                ))
            }
            SmtTokenAction::Approve {
                proof: _,
                mut owner_account,
                spender,
                amount,
            } => {
                let owner_key = owner_account.get_key();
                owner_account.update_allowance(spender.clone(), amount);
                if let Err(e) = self.accounts.update(owner_key, owner_account) {
                    return Err(format!("Failed to update owner account: {e}"));
                }
                Ok(format!("Approved {} to {}", amount, spender))
            }
        };
        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, execution_ctx, vec![])),
        }
    }

    fn commit(&self) -> sdk::StateCommitment {
        sdk::StateCommitment(self.accounts.root().as_slice().to_vec())
    }
}

impl SmtTokenState {
    pub fn get_state(&self) -> HashMap<String, Account> {
        self.accounts
            .store()
            .leaves_map()
            .iter()
            .map(|(_, account)| (account.address.clone(), account.clone()))
            .collect()
    }
}

impl BorshSerialize for SmtTokenState {
    fn serialize<W: std::io::Write>(&self, _writer: &mut W) -> std::io::Result<()> {
        unreachable!("This function should not be called");
        // self.accounts.root().as_slice().serialize(writer)?;
        // Ok(())
    }
}

impl BorshDeserialize for SmtTokenState {
    fn deserialize_reader<R: std::io::Read>(_reader: &mut R) -> std::io::Result<Self> {
        unreachable!("This function should not be called");
        // let mut root_hash = [0u8; 32];
        // reader.read_exact(&mut root_hash)?;
        // let accounts = AccountSMT::new(root_hash.into(), DefaultStore::<Account>::default());
        // Ok(Self::new(accounts))
    }
}

impl serde::Serialize for SmtTokenState {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        unreachable!("This function should not be called");
        // let root_hash = self.accounts.root().as_slice();
        // serializer.serialize_bytes(root_hash)
    }
}

impl<'de> serde::Deserialize<'de> for SmtTokenState {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        unreachable!("This function should not be called");
        // struct SmtTokenStateVisitor;

        // impl serde::de::Visitor<'_> for SmtTokenStateVisitor {
        //     type Value = SmtTokenState;

        //     fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        //         formatter.write_str("a 32-byte root hash for the SMT")
        //     }

        //     fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        //     where
        //         E: serde::de::Error,
        //     {
        //         if v.len() != 32 {
        //             return Err(E::invalid_length(v.len(), &self));
        //         }
        //         let mut root_hash = [0u8; 32];
        //         root_hash.copy_from_slice(v);
        //         let accounts =
        //             AccountSMT::new(root_hash.into(), DefaultStore::<Account>::default());
        //         Ok(SmtTokenState::new(accounts))
        //     }
        // }

        // deserializer.deserialize_bytes(SmtTokenStateVisitor)
    }
}
