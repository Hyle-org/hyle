use anyhow::anyhow;
use client_sdk::transaction_builder::{TransactionBuilder, TxBuilder};
use sdk::{
    identity_provider::{IdentityAction, IdentityVerification},
    BlobData, ContractName,
};

use crate::{AccountInfo, Hydentity};

pub struct Builder<'a, 'b>(TxBuilder<'a, 'b, Hydentity>);

impl Hydentity {
    pub fn builder<'a, 'b: 'a>(
        &'a self,
        contract_name: ContractName,
        builder: &'b mut TransactionBuilder,
    ) -> Builder {
        Builder(TxBuilder {
            state: self,
            contract_name,
            builder,
        })
    }
}

impl<'a, 'b> Builder<'a, 'b> {
    pub fn verify_identity(&mut self, password: String) -> anyhow::Result<()> {
        let nonce = self.get_nonce(&self.0.builder.identity.0)?;
        let password = BlobData(password.into_bytes().to_vec());

        self.0.builder.add_action(
            self.0.contract_name.clone(),
            crate::metadata::HYDENTITY_ELF,
            self.0.state.clone(),
            IdentityAction::VerifyIdentity {
                account: self.0.builder.identity.0.clone(),
                nonce,
            },
            password,
            None,
            None,
            None,
        )
    }

    pub fn register_identity(&mut self, password: String) -> anyhow::Result<()> {
        let password = BlobData(password.into_bytes().to_vec());

        self.0.builder.add_action(
            self.0.contract_name.clone(),
            crate::metadata::HYDENTITY_ELF,
            self.0.state.clone(),
            IdentityAction::RegisterIdentity {
                account: self.0.builder.identity.0.clone(),
            },
            password,
            None,
            None,
            None,
        )
    }

    fn get_nonce(&self, username: &str) -> anyhow::Result<u32> {
        let info = self
            .0
            .state
            .get_identity_info(username)
            .map_err(|e| anyhow!(e))?;
        let state: AccountInfo = serde_json::from_str(&info)?;
        Ok(state.nonce)
    }
}
