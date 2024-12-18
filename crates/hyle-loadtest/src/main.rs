use goose::prelude::*;
use hyle::{
    model::{ContractName, ProofTransaction},
    tools::{contract_runner::ContractRunner, transactions_builder::TransactionBuilder},
};
use hyle_contract_sdk::{flatten_blobs, Blob, BlobIndex, HyleOutput, StateDigest, TxHash};
use uuid::Uuid;

struct UserData {
    identity: String,
    blobs: Vec<Blob>,
    blob_tx_hash: TxHash,
}

async fn register_user(user: &mut GooseUser) -> TransactionResult {
    let user_id = Uuid::new_v4();
    let identity = format!("{}.hydentity", user_id);

    // User builds the register identity transaction
    let mut transaction = TransactionBuilder::new(identity.clone().into());
    transaction.register_identity(identity.clone());
    let blob_transaction = transaction.to_blob_transaction();

    // User sends the register identity transaction
    let goose_response = user
        .post_json("/v1/tx/send/blob", &blob_transaction)
        .await?;

    let blob_tx_hash: TxHash = goose_response
        .response
        .unwrap()
        .text()
        .await
        .unwrap()
        .into();

    user.set_session_data(UserData {
        identity,
        blobs: blob_transaction.blobs,
        blob_tx_hash,
    });

    Ok(())
}

async fn prove_register_user(user: &mut GooseUser) -> TransactionResult {
    let UserData {
        ref identity,
        ref blobs,
        ref blob_tx_hash,
    } = user.get_session_data_unchecked::<UserData>();

    let reqwest_request_builder = user.get_request_builder(&GooseMethod::Get, "/")?;

    let initial_state = StateDigest::default();
    let next_state = StateDigest::default();

    let hyle_output = HyleOutput {
        version: 1,
        initial_state,
        next_state,
        identity: identity.to_owned().into(),
        tx_hash: blob_tx_hash.clone(),
        index: BlobIndex(0),
        blobs: flatten_blobs(blobs),
        success: true,
        program_outputs: vec![],
    };

    let proof = ContractRunner::prove_test(hyle_output).unwrap();

    let register_identity_proof_transaction = ProofTransaction {
        blob_tx_hash: blob_tx_hash.clone(),
        proof,
        contract_name: ContractName("hydentity".to_owned()),
    };
    user.post_json("/v1/tx/send/proof", &register_identity_proof_transaction)
        .await?;
    Ok(())
}

/// Scenario for a User
/// User sends BlobTx to register its identity
/// User gets the state of the identity contract
/// User computes the identity contract to get correct initial/final states
/// User sends ProofTx to prove the registration of its identity
/// User sends BlobTx to swap tokens
/// User gets the state of the AMM contract
/// User computes the AMM contract to get correct initial/final states
/// User send ProofTx to prove his swap
#[tokio::main]
async fn main() -> Result<(), GooseError> {
    // Pour le setup: on crée un contrat AMM
    // On crée deux tokens en s'assurant que tous les Users ont des tokens
    // On crée une nouvelle paire pour ces deux token sur l'AMM
    GooseAttack::initialize()?
        .register_scenario(
            scenario!("HyleBenchmark")
                .register_transaction(transaction!(register_user))
                .register_transaction(transaction!(prove_register_user)),
        )
        .execute()
        .await?;

    Ok(())
}
