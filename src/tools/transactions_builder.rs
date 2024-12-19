use std::pin::Pin;

use anyhow::{anyhow, bail, Error, Result};
use hydentity::{AccountInfo, Hydentity};
use hyle_contract_sdk::{
    caller::ExecutionContext,
    erc20::ERC20Action,
    identity_provider::{IdentityAction, IdentityVerification},
    Blob, BlobData, BlobIndex, ContractName, Digestable, HyleOutput, Identity,
};
use hyllar::HyllarToken;
use staking::{model::ValidatorPublicKey, state::Staking, StakingAction, StakingContract};
use tracing::info;

use crate::model::ProofData;

use super::contract_runner::ContractRunner;

pub struct Password(BlobData);

pub static HYLLAR_BIN: &[u8] = hyle_contracts::HYLLAR_ELF;
pub static HYDENTITY_BIN: &[u8] = hyle_contracts::HYDENTITY_ELF;
pub static STAKING_BIN: &[u8] = hyle_contracts::STAKING_ELF;

pub fn get_binary(contract_name: ContractName) -> Result<&'static [u8]> {
    match contract_name.0.as_str() {
        "hyllar" => Ok(HYLLAR_BIN),
        "hydentity" => Ok(HYDENTITY_BIN),
        "staking" => Ok(STAKING_BIN),
        _ => bail!("contract {} not supported", contract_name),
    }
}

#[derive(Debug, Clone)]
pub struct States {
    pub hyllar: HyllarToken,
    pub hydentity: Hydentity,
    pub staking: Staking,
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

pub struct TransactionBuilder {
    pub identity: Identity,
    hydentity_cf: Vec<(IdentityAction, Password, BlobIndex)>,
    hyllar_cf: Vec<(ERC20Action, ContractName, BlobIndex)>,
    staking_cf: Vec<(StakingAction, BlobIndex)>,
    blobs: Vec<Blob>,
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
            hydentity_cf: vec![],
            hyllar_cf: vec![],
            staking_cf: vec![],
            blobs: vec![],
            runners: vec![],
        }
    }

    fn add_hydentity_cf(&mut self, action: IdentityAction, password: Password) {
        self.hydentity_cf
            .push((action.clone(), password, BlobIndex(self.blobs.len())));
        self.blobs.push(action.as_blob("hydentity".into()));
    }
    fn add_hyllar_cf(
        &mut self,
        token: ContractName,
        action: ERC20Action,
        caller: Option<BlobIndex>,
    ) {
        self.hyllar_cf
            .push((action.clone(), token.clone(), BlobIndex(self.blobs.len())));
        self.blobs.push(action.as_blob(token, caller, None));
    }
    fn add_stake_cf(&mut self, action: StakingAction) {
        self.staking_cf
            .push((action.clone(), BlobIndex(self.blobs.len())));
        self.blobs
            .push(action.as_blob("staking".into(), None, None));
    }

    pub async fn verify_identity(
        &mut self,
        state: &Hydentity,
        password: String,
    ) -> Result<(), Error> {
        let nonce = get_nonce(state, &self.identity.0).await?;
        let password = Password(BlobData(password.into_bytes().to_vec()));

        self.add_hydentity_cf(
            IdentityAction::VerifyIdentity {
                account: self.identity.0.clone(),
                nonce,
            },
            password,
        );

        Ok(())
    }

    pub fn register_identity(&mut self, password: String) {
        let password = Password(BlobData(password.into_bytes().to_vec()));

        self.add_hydentity_cf(
            IdentityAction::RegisterIdentity {
                account: self.identity.0.clone(),
            },
            password,
        );
    }

    pub fn approve(&mut self, token: ContractName, spender: String, amount: u128) {
        self.add_hyllar_cf(token, ERC20Action::Approve { spender, amount }, None);
    }

    pub fn transfer(&mut self, token: ContractName, recipient: String, amount: u128) {
        self.add_hyllar_cf(token, ERC20Action::Transfer { recipient, amount }, None);
    }

    pub fn stake(
        &mut self,
        token: ContractName,
        staking_contract: ContractName,
        amount: u128,
    ) -> Result<(), Error> {
        self.add_stake_cf(StakingAction::Stake { amount });
        self.add_hyllar_cf(
            token,
            ERC20Action::Transfer {
                recipient: staking_contract.0.clone(),
                amount,
            },
            None,
        );

        Ok(())
    }

    pub fn delegate(&mut self, validator: ValidatorPublicKey) -> Result<(), Error> {
        self.add_stake_cf(StakingAction::Delegate { validator });

        Ok(())
    }

    pub async fn build(&mut self, states: &mut States) -> Result<BuildResult> {
        let mut new_states = states.clone();
        let mut outputs = vec![];
        for id in self.hydentity_cf.iter() {
            let runner = ContractRunner::new(
                "hydentity".into(),
                get_binary("hydentity".into())?,
                self.identity.clone(),
                id.1 .0.clone(),
                self.blobs.clone(),
                id.2.clone(),
                new_states.hydentity.clone(),
            )
            .await?;
            let out = runner.execute()?;
            new_states.hydentity = out.next_state.clone().try_into()?;
            outputs.push(("hydentity".into(), out));
            self.runners.push(runner);
        }

        for cf in self.hyllar_cf.iter() {
            let runner = ContractRunner::new(
                cf.1.clone(),
                get_binary(cf.1.clone())?,
                self.identity.clone(),
                BlobData(vec![]),
                self.blobs.clone(),
                cf.2.clone(),
                new_states.for_token(&cf.1)?.clone(),
            )
            .await?;
            let out = runner.execute()?;
            new_states.update_for_token(&cf.1, out.next_state.clone().try_into()?)?;
            outputs.push(("hyllar".into(), out));
            self.runners.push(runner);
        }

        for cf in self.staking_cf.iter() {
            info!("State before runner: {:?}", new_states.staking);
            info!("on chain state: {:?}", new_states.staking.on_chain_state());
            let runner = ContractRunner::new::<staking::state::OnChainState>(
                "staking".into(),
                get_binary("staking".into())?,
                self.identity.clone(),
                BlobData(new_states.staking.as_digest().0),
                self.blobs.clone(),
                cf.1.clone(),
                new_states.staking.on_chain_state(),
            )
            .await?;
            let out = runner.execute()?;
            let mut contract = StakingContract::new(
                ExecutionContext {
                    callees_blobs: vec![].into(),
                    caller: self.identity.clone(),
                },
                new_states.staking.clone(),
            );

            contract
                .execute_action(cf.0.clone())
                .map_err(|e| anyhow!("Error in staking execution: {e}"))?;

            new_states.staking = contract.state();
            assert_eq!(
                out.next_state,
                new_states.staking.on_chain_state().as_digest()
            );
            outputs.push(("staking".into(), out));
            self.runners.push(runner);
        }

        *states = new_states;

        Ok(BuildResult {
            identity: self.identity.clone(),
            blobs: self.blobs.clone(),
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
