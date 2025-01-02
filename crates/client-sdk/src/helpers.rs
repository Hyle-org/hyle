use anyhow::{bail, Result};
use sdk::{ContractInput, HyleOutput};

use crate::ProofData;

#[allow(dead_code)]
fn prove(binary: Vec<u8>, input: ContractInput) -> Result<(ProofData, HyleOutput)> {
    let env = risc0_zkvm::ExecutorEnv::builder()
        .write(&input)
        .unwrap()
        .build()
        .unwrap();

    let prover = risc0_zkvm::default_prover();

    let receipt = prover.prove(env, &binary).unwrap().receipt;

    let hyle_output = receipt
        .journal
        .decode::<HyleOutput>()
        .expect("Failed to decode journal");

    if !hyle_output.success {
        let program_error = std::str::from_utf8(&hyle_output.program_outputs).unwrap();
        println!(
            "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
            program_error
        );
        bail!("Execution failed");
    }

    let proof = ProofData::Bytes(borsh::to_vec(&receipt).expect("Unable to encode receipt"));
    Ok((proof, hyle_output))
}
