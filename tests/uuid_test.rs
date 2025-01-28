#![allow(clippy::unwrap_used, clippy::expect_used)]
use client_sdk::{
    contract_states,
    helpers::risc0::Risc0Prover,
    transaction_builder::{ProvableBlobTx, TxExecutorBuilder},
};
use fixtures::ctx::{E2EContract, E2ECtx};
use hydentity::{client::register_identity, Hydentity};
use hyle::mempool::verifiers::verify_proof;
use hyle_contract_sdk::{
    flatten_blobs, BlobIndex, BlobTransaction, ContractName, Digestable, Hashable, HyleOutput,
    ProgramId, RegisterContractEffect, StateDigest, Transaction, Verifier,
};
use hyle_contracts::{HYDENTITY_ELF, UUID_TLD_ELF, UUID_TLD_ID};
use uuid_tld::{RegisterUuidContract, UuidTldState};

contract_states!(
    struct States {
        uuid: UuidTldState,
        hydentity: Hydentity,
    }
);

mod fixtures;

struct UuidContract {}
impl E2EContract for UuidContract {
    fn verifier() -> Verifier {
        Verifier("risc0".into())
    }
    fn program_id() -> ProgramId {
        ProgramId(UUID_TLD_ID.to_vec())
    }
    fn state_digest() -> StateDigest {
        UuidTldState::default().as_digest()
    }
}

#[test_log::test(tokio::test)]
#[should_panic]
async fn test_uuid_registration() {
    std::env::set_var("RISC0_DEV_MODE", "1");

    let ctx = E2ECtx::new_single(500).await.unwrap();
    ctx.register_contract::<UuidContract>("hyle.hyle".into(), "uuid")
        .await
        .unwrap();

    let mut executor = TxExecutorBuilder::new(States {
        uuid: UuidTldState::default(),
        hydentity: ctx
            .get_contract("hydentity")
            .await
            .unwrap()
            .state
            .try_into()
            .unwrap(),
    })
    .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
    .with_prover("uuid".into(), Risc0Prover::new(UUID_TLD_ELF))
    .build();

    let mut tx = ProvableBlobTx::new("toto.hydentity".into());

    register_identity(&mut tx, "hydentity".into(), "password".into()).unwrap();

    tx.add_action(
        "uuid".into(),
        RegisterUuidContract {
            verifier: "test".into(),
            program_id: ProgramId(vec![]),
            state_digest: StateDigest(vec![0, 1, 2, 3]),
        },
        None,
        None,
    )
    .unwrap()
    .with_private_input(|state| Ok(state.0));

    ctx.send_provable_blob_tx(&tx).await.unwrap();

    let blob_tx = BlobTransaction {
        identity: tx.identity.clone(),
        blobs: tx.blobs.clone(),
    };
    let mut proof = executor.process(tx).unwrap().iter_prove();

    let first_proof = proof.next().unwrap().0.await.unwrap();
    let uuid_proof = proof.next().unwrap().0.await.unwrap();

    ctx.send_proof_single("hydentity".into(), first_proof.clone(), blob_tx.hash())
        .await
        .unwrap();
    ctx.send_proof_single("uuid".into(), uuid_proof.clone(), blob_tx.hash())
        .await
        .unwrap();

    let outputs = verify_proof(
        &uuid_proof,
        &Verifier("risc0".into()),
        &ProgramId(UUID_TLD_ID.to_vec()),
    )
    .expect("Must validate proof");

    let expected_output = RegisterContractEffect {
        contract_name: "5f44a3f5-c5f4-4a40-a1d4-5176a2602600.uuid".into(),
        verifier: Verifier("test".into()),
        program_id: ProgramId(vec![]),
        state_digest: StateDigest(vec![0, 1, 2, 3]),
    };

    let blobs = flatten_blobs(&blob_tx.blobs);
    assert_eq!(
        outputs,
        vec![HyleOutput {
            version: 1,
            initial_state: UuidTldState::default().as_digest(),
            next_state: executor.uuid.as_digest(),
            identity: "toto.hydentity".into(),
            tx_hash: Into::<Transaction>::into(blob_tx).hash(),
            tx_ctx: None,
            index: BlobIndex(1),
            blobs,
            success: true,
            registered_contracts: vec![expected_output],
            program_outputs: vec![]
        }]
    );

    let contract = loop {
        if let Ok(c) = ctx
            .get_contract("5f44a3f5-c5f4-4a40-a1d4-5176a2602600.uuid")
            .await
        {
            break c;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    assert_eq!(
        contract.name,
        "5f44a3f5-c5f4-4a40-a1d4-5176a2602600.uuid".into()
    );
    assert_eq!(contract.verifier, Verifier("test".into()));
    assert_eq!(contract.state, StateDigest(vec![0, 1, 2, 3]));
}
