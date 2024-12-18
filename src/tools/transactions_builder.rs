use std::pin::Pin;

use amm::AmmAction;
use anyhow::{bail, Error, Result};
use hydentity::{AccountInfo, Hydentity};
use hyle_contract_sdk::{
    erc20::ERC20Action,
    identity_provider::{IdentityAction, IdentityVerification},
    Blob, BlobData, BlobIndex, ContractAction, ContractName, HyleOutput, Identity,
};
use hyllar::HyllarToken;

use crate::model::{BlobTransaction, ProofData};

use super::contract_runner::ContractRunner;

pub static HYLLAR_BIN: &[u8] = include_bytes!("../../contracts/hyllar/hyllar.img");
pub static HYDENTITY_BIN: &[u8] = include_bytes!("../../contracts/hydentity/hydentity.img");

pub fn get_binary(contract_name: ContractName) -> Result<&'static [u8]> {
    match contract_name.0.as_str() {
        "hyllar" => Ok(HYLLAR_BIN),
        "hydentity" => Ok(HYDENTITY_BIN),
        _ => bail!("contract {} not supported", contract_name),
    }
}

#[derive(Debug, Clone)]
pub struct States {
    pub hyllar: HyllarToken,
    pub hydentity: Hydentity,
}

impl States {
    pub fn for_token<'a>(&'a self, token: &ContractName) -> Result<&'a HyllarToken> {
        match token.0.as_str() {
            "hyllar" => Ok(&self.hyllar),
            _ => bail!("Invalid token"),
        }
    }

    pub fn update_for_token(&mut self, token: &ContractName, new_state: HyllarToken) -> Result<()> {
        match token.0.as_str() {
            "hyllar" => self.hyllar = new_state,
            _ => bail!("Invalid token"),
        }
        Ok(())
    }
}

pub struct BlobAction<T: ContractAction> {
    action: T,                       // action
    contract_name: ContractName,     // contract name
    private_input: Option<BlobData>, // private inputs
    caller: Option<BlobIndex>,       // caller
    callees: Option<Vec<BlobIndex>>, // callees
}

impl<T: ContractAction> BlobAction<T> {
    pub fn as_blob(&self) -> Blob {
        self.action.as_blob(
            self.contract_name.clone(),
            self.caller.clone(),
            self.callees.clone(),
        )
    }
}

// enum SupportedContractActionEnum {
//     ERC20Action(ERC20Action),
//     IdentityAction(IdentityAction),
//     AmmAction(AmmAction),
// }

// Problème pour des T differents au sein d'une même transaction
pub struct TransactionBuilder<T: ContractAction> {
    pub identity: Identity,
    actions: Vec<BlobAction<T>>,
    runners: Vec<ContractRunner>,
}

pub struct BuildResult {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    pub outputs: Vec<(ContractName, HyleOutput)>,
}

impl<T: ContractAction + Clone> TransactionBuilder<T> {
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            actions: vec![],
            runners: vec![],
        }
    }

    pub fn to_blob_transaction(&self) -> BlobTransaction
    where
        T: ContractAction,
    {
        let blobs = self
            .actions
            .iter()
            .map(|blob_action| blob_action.as_blob())
            .collect();
        BlobTransaction {
            identity: self.identity.clone(),
            blobs,
        }
    }

    fn add_action(
        &mut self,
        action: T,
        contract_name: ContractName,
        private_input: Option<BlobData>,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) where
        T: ContractAction,
    {
        let action = BlobAction {
            action,
            contract_name,
            private_input,
            caller,
            callees,
        };
        self.actions.push(action);
    }

    pub fn register_identity(&mut self, password: String)
    where
        T: From<IdentityAction> + ContractAction,
    {
        let password = BlobData(password.into_bytes().to_vec());

        self.add_action(
            IdentityAction::RegisterIdentity {
                account: self.identity.0.clone(),
            }
            .into(),
            "hydentity".into(),
            Some(password),
            None,
            None,
        );
    }

    pub async fn verify_identity(
        &mut self,
        state: &Hydentity,
        password: String,
    ) -> Result<(), Error>
    where
        T: From<IdentityAction> + ContractAction,
    {
        let nonce = get_nonce(state, &self.identity.0).await?;
        let password = BlobData(password.into_bytes().to_vec());

        self.add_action(
            IdentityAction::VerifyIdentity {
                account: self.identity.0.clone(),
                nonce,
            }
            .into(),
            "hydentity".into(),
            Some(password),
            None,
            None,
        );

        Ok(())
    }

    pub fn approve(&mut self, token: ContractName, spender: String, amount: u128)
    where
        T: From<ERC20Action> + ContractAction,
    {
        self.add_action(
            ERC20Action::Approve { spender, amount }.into(),
            token,
            None,
            None,
            None,
        );
    }

    pub fn transfer(&mut self, token: ContractName, recipient: String, amount: u128)
    where
        T: From<ERC20Action> + ContractAction,
    {
        self.add_action(
            ERC20Action::Transfer { recipient, amount }.into(),
            token,
            None,
            None,
            None,
        );
    }

    pub async fn build(&mut self, states: &mut States) -> Result<BuildResult>
    where
        T: Clone,
    {
        let mut new_states = states.clone();
        let mut outputs = vec![];

        let blob_transaction = self.to_blob_transaction();

        for (i, blob_action) in self.actions.iter().enumerate() {
            let contract_name = blob_action.contract_name.clone();
            let private_input = blob_action.private_input.clone();
            let blob_index = BlobIndex(i);

            let runner = ContractRunner::new(
                contract_name.clone(),
                get_binary(contract_name.clone())?,
                self.identity.clone(),
                private_input.unwrap_or(BlobData(vec![])),
                blob_transaction.blobs.clone(),
                blob_index,
                new_states.for_token(&contract_name)?.clone(),
            )
            .await?;
            let out = runner.execute()?;
            new_states.hydentity = out.next_state.clone().try_into()?;
            outputs.push(("hydentity".into(), out));
            self.runners.push(runner);
        }

        *states = new_states;

        Ok(BuildResult {
            identity: self.identity.clone(),
            blobs: blob_transaction.blobs.clone(),
            outputs,
        })
    }

    /// Returns an iterator over the proofs of the transactions
    /// In order to send proofs when they are ready, without waiting for all of them to be ready
    /// Example usage:
    /// for (proof, contract_name) in transaction.iter_prove() {
    ///    let proof: ProofData = proof.await.unwrap();
    ///    ctx.client()
    ///        .send_tx_proof(&hyle::model::ProofTransaction {
    ///            blob_tx_hash: blob_tx_hash.clone(),
    ///            proof,
    ///            contract_name,
    ///        })
    ///        .await
    ///        .unwrap();
    ///}
    pub fn iter_prove<'a>(
        &'a self,
    ) -> impl Iterator<
        Item = (
            Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + 'a>>,
            ContractName,
        ),
    > + 'a {
        self.runners.iter().map(|runner| {
            let future = runner.prove();
            (
                Box::pin(future)
                    as Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + 'a>>,
                runner.contract_name.clone(),
            )
        })
    }
}

async fn get_nonce(state: &Hydentity, username: &str) -> Result<u32, Error> {
    let info = state
        .get_identity_info(username)
        .map_err(|err| anyhow::anyhow!(err))?;
    let state: AccountInfo = serde_json::from_str(&info)
        .map_err(|_| anyhow::anyhow!("Failed to parse identity info"))?;
    Ok(state.nonce)
}
