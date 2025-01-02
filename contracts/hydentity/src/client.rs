use client_sdk::transaction_builder::TransactionBuilder;
use sdk::{identity_provider::IdentityAction, BlobData, ContractName, Digestable};

use crate::Hydentity;

pub struct Builder<'b> {
    pub contract_name: ContractName,
    pub builder: &'b mut TransactionBuilder,
}

impl Hydentity {
    pub fn default_builder<'b>(&self, builder: &'b mut TransactionBuilder) -> Builder<'b> {
        builder.init_with("hydentity".into(), self.as_digest());
        Builder {
            contract_name: "hydentity".into(),
            builder,
        }
    }
}

impl<'b> Builder<'b> {
    pub fn verify_identity(&mut self, state: &Hydentity, password: String) -> anyhow::Result<()> {
        let nonce = state
            .get_nonce(self.builder.identity.0.as_str())
            .map_err(|e| anyhow::anyhow!(e))?;

        let password = BlobData(password.into_bytes().to_vec());

        self.builder
            .add_action(
                self.contract_name.clone(),
                crate::metadata::HYDENTITY_ELF,
                IdentityAction::VerifyIdentity {
                    account: self.builder.identity.0.clone(),
                    nonce,
                },
                None,
                None,
            )?
            .with_private_blob(move |_| Ok(password.clone()));
        Ok(())
    }

    pub fn register_identity(&mut self, password: String) -> anyhow::Result<()> {
        let password = BlobData(password.into_bytes().to_vec());

        self.builder
            .add_action(
                self.contract_name.clone(),
                crate::metadata::HYDENTITY_ELF,
                IdentityAction::RegisterIdentity {
                    account: self.builder.identity.0.clone(),
                },
                None,
                None,
            )?
            .with_private_blob(move |_| Ok(password.clone()));
        Ok(())
    }
}
