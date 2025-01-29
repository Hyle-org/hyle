use client_sdk::{
    contract_states,
    transaction_builder::{ProvableBlobTx, TxExecutorBuilder},
};
use hydentity::{client::register_identity, Hydentity};
use risc0_recursion::ProofInput;
use sdk::{ContractName, Digestable, HyleOutput, StateDigest};

contract_states!(
    struct States {
        hydentity: Hydentity,
    }
);

#[test_log::test(tokio::test)]
async fn test_recursion() {
    std::env::set_var("RISC0_DEV_MODE", "1");

    let mut executor = TxExecutorBuilder::new(States {
        hydentity: Hydentity::new(),
    })
    .build();

    let mut tx = ProvableBlobTx::new("bob.hydentity".into());
    register_identity(&mut tx, "hydentity".into(), "password".into()).unwrap();
    let first_proof = executor
        .process(tx)
        .unwrap()
        .iter_prove()
        .next()
        .unwrap()
        .await
        .unwrap()
        .proof;

    let mut tx = ProvableBlobTx::new("alice.hydentity".into());
    register_identity(&mut tx, "hydentity".into(), "password".into()).unwrap();
    let second_proof = executor
        .process(tx)
        .unwrap()
        .iter_prove()
        .next()
        .unwrap()
        .await
        .unwrap()
        .proof;

    let first_receipt = borsh::from_slice::<risc0_zkvm::Receipt>(first_proof.0.as_slice())
        .expect("Failed to decode first receipt");
    let second_receipt = borsh::from_slice::<risc0_zkvm::Receipt>(second_proof.0.as_slice())
        .expect("Failed to decode second receipt");

    let env = risc0_zkvm::ExecutorEnv::builder()
        .add_assumption(first_receipt.clone())
        .add_assumption(second_receipt.clone())
        .write(&vec![
            ProofInput {
                image_id: hyle_contracts::HYDENTITY_ID,
                journal: first_receipt.journal.bytes,
            },
            ProofInput {
                image_id: hyle_contracts::HYDENTITY_ID,
                journal: second_receipt.journal.bytes,
            },
        ])
        .unwrap()
        .build()
        .unwrap();

    let receipt = risc0_zkvm::default_prover()
        .prove(env, hyle_contracts::RISC0_RECURSION_ELF)
        .unwrap()
        .receipt;

    receipt.verify(hyle_contracts::RISC0_RECURSION_ID).unwrap();

    let outputs: Vec<([u8; 32], Vec<u8>)> =
        receipt.journal.decode().expect("Failed to decode journal");
    let outputs = outputs
        .iter()
        .map(|x| {
            (
                x.0,
                risc0_zkvm::serde::from_slice::<HyleOutput, _>(&x.1).unwrap(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(outputs.first().unwrap().0, hyle_contracts::HYDENTITY_ID,);
    assert_eq!(
        outputs.first().unwrap().1.program_outputs,
        b"Successfully registered identity for account: bob.hydentity".to_vec()
    );
    assert_eq!(outputs.last().unwrap().0, hyle_contracts::HYDENTITY_ID,);
    assert_eq!(
        outputs.last().unwrap().1.program_outputs,
        b"Successfully registered identity for account: alice.hydentity".to_vec()
    );

    assert_eq!(outputs.len(), 2);
}
