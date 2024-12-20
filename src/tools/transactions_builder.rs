use std::pin::Pin;

use amm::AmmAction;
use anyhow::{bail, Error, Result};
use hydentity::{AccountInfo, Hydentity};
use hyle_contract_sdk::{
    erc20::ERC20Action,
    identity_provider::{IdentityAction, IdentityVerification},
    Blob, BlobData, BlobIndex, ContractAction, ContractName, HyleOutput, Identity, StateDigest,
};
use hyllar::HyllarToken;

use crate::model::{BlobTransaction, ProofData, RegisterContractTransaction};

use super::contract_runner::ContractRunner;

pub static HYLLAR_BIN: &[u8] = include_bytes!("../../contracts/hyllar/hyllar.img");
pub static HYDENTITY_BIN: &[u8] = include_bytes!("../../contracts/hydentity/hydentity.img");
pub static AMM_BIN: &[u8] = include_bytes!("../../contracts/amm/amm.img");

pub fn get_binary(contract_name: ContractName) -> Result<&'static [u8]> {
    match contract_name.0.as_str() {
        "hyllar" => Ok(HYLLAR_BIN),
        "hydentity" => Ok(HYDENTITY_BIN),
        "amm" => Ok(AMM_BIN),
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

pub struct BlobAction {
    action: Box<dyn ContractAction>, // action
    contract_name: ContractName,     // contract name
    private_input: Option<BlobData>, // private inputs
    caller: Option<BlobIndex>,       // caller
    callees: Option<Vec<BlobIndex>>, // callees
}

impl BlobAction {
    pub fn as_blob(&self) -> Blob {
        self.action.as_blob(
            self.contract_name.clone(),
            self.caller.clone(),
            self.callees.clone(),
        )
    }
}

pub struct TransactionBuilder {
    pub identity: Identity,
    actions: Vec<BlobAction>,
    runners: Vec<ContractRunner>,
}

pub struct BuildResult {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    pub outputs: Vec<(ContractName, HyleOutput)>,
}

impl TransactionBuilder {
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            actions: vec![],
            runners: vec![],
        }
    }

    pub fn register_contract(
        owner: &str,
        verifier: &str,
        program_id: &[u8],
        state_digest: StateDigest,
        contract_name: &str,
    ) -> RegisterContractTransaction {
        RegisterContractTransaction {
            owner: owner.into(),
            verifier: verifier.into(),
            program_id: program_id.into(),
            state_digest,
            contract_name: contract_name.into(),
        }
    }

    pub fn to_blob_transaction(&self) -> BlobTransaction {
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
        action: Box<dyn ContractAction>,
        contract_name: ContractName,
        private_input: Option<BlobData>,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) {
        let action = BlobAction {
            action,
            contract_name,
            private_input,
            caller,
            callees,
        };
        self.actions.push(action);
    }

    pub fn register_identity(&mut self, contract_name: ContractName, password: String) {
        let password = BlobData(password.into_bytes().to_vec());

        self.add_action(
            Box::new(IdentityAction::RegisterIdentity {
                account: self.identity.0.clone(),
            }),
            contract_name,
            Some(password),
            None,
            None,
        );
    }

    pub async fn verify_identity(
        &mut self,
        state: &Hydentity,
        contract_name: ContractName,
        password: String,
    ) -> Result<(), Error> {
        let nonce = get_nonce(state, &self.identity.0).await?;
        let password = BlobData(password.into_bytes().to_vec());

        self.add_action(
            Box::new(IdentityAction::VerifyIdentity {
                account: self.identity.0.clone(),
                nonce,
            }),
            contract_name,
            Some(password),
            None,
            None,
        );

        Ok(())
    }

    pub fn approve(&mut self, token: ContractName, spender: String, amount: u128) {
        self.add_action(
            Box::new(ERC20Action::Approve { spender, amount }),
            token,
            None,
            None,
            None,
        );
    }

    pub fn transfer(
        &mut self,
        token: ContractName,
        recipient: String,
        amount: u128,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) {
        self.add_action(
            Box::new(ERC20Action::Transfer { recipient, amount }),
            token,
            None,
            caller,
            callees,
        );
    }

    pub fn transfer_from(
        &mut self,
        token: ContractName,
        sender: String,
        recipient: String,
        amount: u128,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) {
        self.add_action(
            Box::new(ERC20Action::TransferFrom {
                sender,
                recipient,
                amount,
            }),
            token,
            None,
            caller,
            callees,
        );
    }

    pub fn swap(
        &mut self,
        sender: String,
        amm_contract: ContractName,
        pair: (String, String),
        amounts: (u128, u128),
    ) {
        let latest_blob_index = self.actions.len();
        let callees = Some(vec![
            BlobIndex(latest_blob_index + 2),
            BlobIndex(latest_blob_index + 3),
        ]);
        self.add_action(
            Box::new(AmmAction::Swap {
                pair: pair.clone(),
                amounts,
            }),
            amm_contract.clone(),
            None,
            None,
            callees,
        );

        // TransferFrom to the amm
        self.transfer_from(
            pair.0.into(),
            sender.clone(),
            amm_contract.0.clone(),
            amounts.0,
            Some(BlobIndex(latest_blob_index + 1)),
            None,
        );

        // Transfer to the sender
        self.transfer(
            pair.1.into(),
            sender,
            amounts.1,
            Some(BlobIndex(latest_blob_index + 1)),
            None,
        );
    }

    // TODO: make it generic
    pub fn build(&mut self, states: &mut States) -> Result<BuildResult> {
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
            )?;
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
