use client_sdk::{
    contract_states,
    transaction_builder::{ProvableBlobTx, TxExecutorBuilder, TxExecutorHandler},
};
use hydentity::{client::tx_executor_handler::register_identity, Hydentity};
use hyle_risc0_recursion::ProofInput;
use sdk::{Blob, Calldata, ContractName, HyleOutput};

contract_states!(
    struct States {
        hydentity: Hydentity,
    }
);
#[test_log::test(tokio::test)]
async fn test_recursion() {
    std::env::set_var("RISC0_DEV_MODE", "1");

    let mut executor = TxExecutorBuilder::new(States {
        hydentity: Hydentity::default(),
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
        String::from_utf8(outputs.first().unwrap().1.program_outputs.clone()).unwrap(),
        "Successfully registered identity for account: bob.hydentity:f7b8be921888c7144023f86a6564ec282de0b18a2407d994f5d962b79afe25db"
    );
    assert_eq!(outputs.last().unwrap().0, hyle_contracts::HYDENTITY_ID,);
    assert_eq!(
        String::from_utf8(outputs.last().unwrap().1.program_outputs.clone()).unwrap(),
        "Successfully registered identity for account: alice.hydentity:a57d2f2aab5d16e86f7c6df0af27724798aa52517943b5f96aebd8b9e40a6d1b"
    );

    assert_eq!(outputs.len(), 2);
}
