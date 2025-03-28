use borsh::{BorshDeserialize, BorshSerialize};
use passport::PassportAction;
use sdk::utils::parse_contract_input;
use sdk::{
    Blob, BlobData, BlobIndex, ContractAction, ContractInput, ContractName, Identity,
    StructuredBlobData,
};
use sdk::{HyleContract, RunResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::vec;

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod indexer;

impl HyleContract for Ponzhyle {
    fn execute(&mut self, contract_input: &ContractInput) -> RunResult {
        let (action, mut execution_ctx) = parse_contract_input::<PonzhyleAction>(contract_input)?;

        let caller = execution_ctx.caller.clone();
        let id_hash = caller
            .to_string()
            .split('.')
            .next()
            .unwrap_or_default()
            .to_string();
        let output = match action {
            PonzhyleAction::RegisterNationality { nationality } => {
                execution_ctx.is_in_callee_blobs(
                    &"passport".into(),
                    PassportAction::VerifyIdentity {
                        id_hash,
                        nationality: nationality.clone(),
                    },
                )?;
                self.register_nationality(&caller, nationality)
            }
            PonzhyleAction::SendInvite { address } => self.refer(&caller, address),
            PonzhyleAction::RedeemInvite { referrer } => self.redeem_invite(&caller, referrer),
        };

        match output {
            Err(e) => Err(e),
            Ok(output) => Ok((output, execution_ctx, vec![])),
        }
    }

    fn commit(&self) -> sdk::StateCommitment {
        let mut hasher = Sha256::new();
        for (identity, account) in self.accounts.iter() {
            hasher.update(identity.0.as_bytes());
            hasher.update(borsh::to_vec(account).expect("Failed to serialize account"));
        }
        sdk::StateCommitment(hasher.finalize().to_vec())
    }
}

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone, Default)]
pub struct Account {
    pub nationality: Option<String>, // None if the passport has not been provided
    pub invites: u32,                // The number of invites this account has
    pub referral: Option<Identity>,  // The one that referred this account
    pub pending_invites: Vec<Identity>, // The accounts that this account referred but that are not registered yet
    pub invitees: Vec<Identity>,        // The accounts invited by this account
}

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone, Default)]
pub struct UserData {
    account: Option<Account>,
    tree: HashMap<Identity, Vec<Identity>>,
}

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Debug, Clone)]
pub struct Ponzhyle {
    accounts: BTreeMap<Identity, Account>,
}

impl Default for Ponzhyle {
    fn default() -> Self {
        let mut accounts = BTreeMap::new();
        let french1 = Identity("1".to_string());
        let french1_invite1 = Identity("11".to_string());
        let french1_invite2 = Identity("12".to_string());
        let french1_invite1_invite1 = Identity("111".to_string());

        let french2 = Identity("2".to_string());
        let french2_invite1 = Identity("21".to_string());
        let french2_invite2 = Identity("22".to_string());
        let french2_invite1_invite1 = Identity("211".to_string());

        // Max
        accounts.insert(
            "Maximilien62549.twitter".into(),
            Account {
                nationality: Some("France".to_string()),
                invites: 10,
                referral: None,
                pending_invites: vec![],
                invitees: vec![],
            },
        );

        // French 1
        accounts.insert(
            french1.clone(),
            Account {
                nationality: Some("France".to_string()),
                invites: 8,
                referral: None,
                pending_invites: vec![],
                invitees: vec![french1_invite1.clone(), french1_invite2.clone()],
            },
        );
        accounts.insert(
            french1_invite1.clone(),
            Account {
                nationality: None,
                invites: 1,
                referral: Some(french1.clone()),
                pending_invites: vec![],
                invitees: vec![french1_invite1_invite1.clone()],
            },
        );
        accounts.insert(
            french1_invite2,
            Account {
                nationality: None,
                invites: 1,
                referral: Some(french1),
                pending_invites: vec![],
                invitees: vec![],
            },
        );
        accounts.insert(
            french1_invite1_invite1,
            Account {
                nationality: None,
                invites: 1,
                referral: Some(french1_invite1),
                pending_invites: vec![],
                invitees: vec![],
            },
        );

        // French 2
        accounts.insert(
            french2.clone(),
            Account {
                nationality: Some("France".to_string()),
                invites: 8,
                referral: None,
                pending_invites: vec![],
                invitees: vec![french2_invite1.clone(), french2_invite2.clone()],
            },
        );
        accounts.insert(
            french2_invite1.clone(),
            Account {
                nationality: None,
                invites: 1,
                referral: Some(french2.clone()),
                pending_invites: vec![],
                invitees: vec![french2_invite1_invite1.clone()],
            },
        );
        accounts.insert(
            french2_invite2.clone(),
            Account {
                nationality: None,
                invites: 1,
                referral: Some(french2),
                pending_invites: vec![],
                invitees: vec![],
            },
        );
        accounts.insert(
            french2_invite1_invite1.clone(),
            Account {
                nationality: None,
                invites: 1,
                referral: Some(french2_invite1),
                pending_invites: vec![],
                invitees: vec![],
            },
        );

        Ponzhyle { accounts }
    }
}

impl Ponzhyle {
    /// Register the nationality of the caller
    /// Will create the account if not registered
    pub fn register_nationality(
        &mut self,
        caller: &Identity,
        nationality: String,
    ) -> Result<String, String> {
        if let Some(account) = self.accounts.get_mut(caller) {
            if account.nationality.is_some() {
                return Err(format!("Nationality already registered for {}", caller));
            }
            account.nationality = Some(nationality);
        } else {
            self.accounts.insert(
                caller.clone(),
                Account {
                    nationality: Some(nationality),
                    ..Default::default()
                },
            );
        }
        Ok(format!("Nationality registered for {}", caller))
    }

    /// The user (caller) refers the address of someone else
    /// The address will be added to the pending referrals of the caller until the owner of that address redeem the referral
    fn refer(&mut self, caller: &Identity, address: Identity) -> Result<String, String> {
        match self.accounts.get_mut(caller) {
            Some(account) => {
                if account.invites - account.pending_invites.len() as u32 == 0 {
                    return Err(format!("{caller} has no invites left"));
                }
                account.pending_invites.push(address.clone());
            }
            None => return Err(format!("{caller} not registered")),
        };
        Ok(format!("Referred {}", address))
    }

    /// A user creates its account.
    /// It will be mandatory that the referral has refered the caller
    fn redeem_invite(&mut self, caller: &Identity, referrer: Identity) -> Result<String, String> {
        if self.accounts.contains_key(caller) {
            return Err(format!("{caller} already registered"));
        }

        if let Some(referrer_account) = self.accounts.get_mut(&referrer) {
            if !referrer_account.pending_invites.contains(caller) {
                return Err(format!("{caller} not referred by {referrer}"));
            }

            referrer_account.pending_invites.retain(|x| x != caller);
            referrer_account.invites -= 1;
            referrer_account.invitees.push(caller.clone());
        } else {
            return Err(format!("Referral {referrer} not found"));
        }

        self.accounts.insert(
            caller.clone(),
            Account {
                nationality: None,
                invites: 3,
                referral: Some(referrer.clone()),
                pending_invites: vec![],
                invitees: vec![],
            },
        );

        Ok(format!("Referral from {referrer} redeemed by {caller}"))
    }

    pub fn get_invitees(&self, identity: &Identity) -> HashMap<Identity, Vec<Identity>> {
        let mut invitees = HashMap::new();
        invitees.insert(identity.clone(), vec![]);

        if let Some(account) = self.accounts.get(identity) {
            if account.invitees.is_empty() {
                return invitees;
            }
            for invite in account.invitees.iter() {
                invitees
                    .entry(identity.clone())
                    .or_insert_with(Vec::new)
                    .push(invite.clone());
                invitees.extend(self.get_invitees(invite));
            }
        }
        invitees
    }

    pub fn get_user_data(&self, identity: &Identity) -> UserData {
        let account = self.accounts.get(identity).cloned();
        let tree = self.get_invitees(identity);
        UserData { account, tree }
    }
}

#[derive(Serialize, Deserialize, BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq)]
pub enum PonzhyleAction {
    SendInvite { address: Identity },
    RedeemInvite { referrer: Identity },
    RegisterNationality { nationality: String },
}

impl ContractAction for PonzhyleAction {
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

#[cfg(test)]
mod tests {
    #[test]
    fn test_register_nationality() {
        // let token = Ponzhyle::default();

        // assert_eq!(token.total_supply, TOTAL_SUPPLY);
        // assert_eq!(
        //     token.balances.get(FAUCET_ID).cloned().unwrap_or(0),
        //     TOTAL_SUPPLY
        // );
        // assert!(token.allowances.is_empty());
    }
}
