use std::collections::HashMap;

use hyrun::{Cli, Context, ContractData};
use risc0_recursion::ProofInput;
use sdk::StateDigest;

#[test_log::test(test)]
fn test_recursion() {
    let temp_dir = tempfile::tempdir().unwrap();

    std::env::set_var("RISC0_DEV_MODE", "1");

    let hardcoded_initial_states = {
        let mut map = HashMap::new();
        map.insert(
            "hydentity".to_owned(),
            StateDigest(
                bincode::encode_to_vec(hydentity::Hydentity::new(), bincode::config::standard())
                    .expect("Failed to encode Hydentity"),
            ),
        );
        map
    };

    hyrun::run_command(&Context {
        cli: Cli {
            command: hyrun::CliCommand::Hydentity {
                command: hyrun::HydentityArgs::Register {
                    account: "toto".to_owned(),
                },
            },
            user: Some("bob".to_owned()),
            password: Some("bob".to_owned()),
            nonce: Some(0),
            host: "".to_owned(),
            port: 0,
            proof_path: temp_dir.path().to_str().unwrap().to_owned(),
        },
        contract_data: ContractData::default(),
        hardcoded_initial_states: hardcoded_initial_states.clone(),
    });
    let first_proof = std::fs::read(format!(
        "{}/0.risc0.proof",
        temp_dir.path().to_str().unwrap()
    ))
    .expect("Failed to read first proof");
    hyrun::run_command(&Context {
        cli: Cli {
            command: hyrun::CliCommand::Hydentity {
                command: hyrun::HydentityArgs::Register {
                    account: "toto".to_owned(),
                },
            },
            user: Some("bob".to_owned()),
            password: Some("bob".to_owned()),
            nonce: Some(0),
            host: "".to_owned(),
            port: 0,
            proof_path: temp_dir.path().to_str().unwrap().to_owned(),
        },
        contract_data: ContractData {
            hydentity_elf: hyle_contracts::HYDENTITY_ELF.to_vec(),
            hydentity_id: hyle_contracts::HYDENTITY_ID.to_vec(),
            ..ContractData::default()
        },
        hardcoded_initial_states,
    });
    let second_proof = std::fs::read(format!(
        "{}/0.risc0.proof",
        temp_dir.path().to_str().unwrap()
    ))
    .expect("Failed to read second proof");

    let first_receipt = borsh::from_slice::<risc0_zkvm::Receipt>(first_proof.as_slice())
        .expect("Failed to decode first receipt");
    let second_receipt = borsh::from_slice::<risc0_zkvm::Receipt>(second_proof.as_slice())
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
}
