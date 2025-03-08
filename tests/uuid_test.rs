#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use client_sdk::{
    contract_states,
    helpers::risc0::Risc0Prover,
    transaction_builder::{ProvableBlobTx, TxExecutorBuilder},
};
use fixtures::ctx::{E2EContract, E2ECtx};

use hydentity::{client::register_identity, Hydentity};
use hyle::mempool::verifiers::verify_proof;
use hyle_contract_sdk::{
    guest, BlobTransaction, ContractInput, ContractName, Digestable, Hashed, HyleOutput, ProgramId,
    StateDigest, Verifier,
};
use hyle_contracts::{HYDENTITY_ELF, UUID_TLD_ELF, UUID_TLD_ID};
use hyle_model::OnchainEffect;
use uuid_tld::{UuidTld, UuidTldAction};

contract_states!(
    struct States {
        uuid: UuidTld,
        hydentity: Hydentity,
    }
);

mod fixtures;

struct UuidContract {}
impl E2EContract for UuidContract {
    fn verifier() -> Verifier {
        Verifier(hyle_verifiers::versions::RISC0_1.to_string())
    }
    fn program_id() -> ProgramId {
        ProgramId(UUID_TLD_ID.to_vec())
    }
    fn state_digest() -> StateDigest {
        UuidTld::default().as_digest()
    }
}

#[test_log::test(tokio::test)]
async fn test_uuid_registration() {
    std::env::set_var("RISC0_DEV_MODE", "1");

    let ctx = E2ECtx::new_multi_with_indexer(2, 500).await.unwrap();
    ctx.register_contract::<UuidContract>("hyle.hyle".into(), "uuid")
        .await
        .unwrap();

    let hydentity: Hydentity = ctx
        .indexer_client()
        .fetch_current_state(&"hydentity".into())
        .await
        .unwrap();

    let mut executor = TxExecutorBuilder::new(States {
        uuid: UuidTld::default(),
        hydentity,
    })
    .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
    .with_prover("uuid".into(), Risc0Prover::new(UUID_TLD_ELF))
    .build();

    let mut tx = ProvableBlobTx::new("toto.hydentity".into());

    register_identity(&mut tx, "hydentity".into(), "password".into()).unwrap();

    tx.add_action(
        "uuid".into(),
        UuidTldAction {
            verifier: "test".into(),
            program_id: ProgramId(vec![]),
            state_digest: StateDigest(vec![0, 1, 2, 3]),
        },
        None,
        None,
        None,
    )
    .unwrap();

    ctx.send_provable_blob_tx(&tx).await.unwrap();

    let blob_tx = BlobTransaction::new(tx.identity.clone(), tx.blobs.clone());

    let tx_context = loop {
        if let Ok(v) = ctx.client().get_unsettled_tx(&blob_tx.hashed()).await {
            break (*v.tx_context).clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    tx.add_context(tx_context.clone());

    // Process TX and note which contract we expect to register.
    let tx = executor.process(tx).unwrap();
    let expected_output = tx.outputs[1].1.clone();

    let mut proofs = tx.iter_prove();
    let first_proof = proofs.next().unwrap().await.unwrap();
    let uuid_proof = proofs.next().unwrap().await.unwrap();

    ctx.send_proof_single(first_proof.clone()).await.unwrap();
    ctx.send_proof_single(uuid_proof.clone()).await.unwrap();

    let outputs = verify_proof(
        &uuid_proof.proof,
        &Verifier(hyle_verifiers::versions::RISC0_1.to_string()),
        &ProgramId(UUID_TLD_ID.to_vec()),
    )
    .expect("Must validate proof");

    assert_eq!(outputs, &[expected_output.clone()]);

    let contract = loop {
        if let Ok(c) = ctx
            .get_contract(match expected_output.onchain_effects.first() {
                Some(OnchainEffect::RegisterContract(e)) => &e.contract_name.0,
                _ => panic!("Expected RegisterContractEffect"),
            })
            .await
        {
            break c;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    assert_eq!(contract.verifier, Verifier("test".into()));
    assert_eq!(contract.state, StateDigest(vec![0, 1, 2, 3]));
}
