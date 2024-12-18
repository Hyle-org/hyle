use goose::prelude::*;
use hyle::model::{BlobTransaction, ContractName};
use hyle_contract_sdk::identity_provider::IdentityAction;

async fn register_user(user: &mut GooseUser) -> TransactionResult {
    for i in 0..10000 {
        let identity = format!("identity-{}.hydentity", i);

        // Register identity
        let blobs = vec![IdentityAction::RegisterIdentity {
            account: identity.clone(),
        }
        .as_blob(ContractName("hydentity".to_owned()))];

        let register_identity_blob_transaction = BlobTransaction {
            identity: identity.into(),
            blobs,
        };
        user.post_json("/v1/tx/send/blob", &register_identity_blob_transaction)
            .await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), GooseError> {
    GooseAttack::initialize()?
        .register_scenario(
            scenario!("RegisterUsersBlobTransactions")
                .register_transaction(transaction!(register_user)),
        )
        .execute()
        .await?;

    Ok(())
}
