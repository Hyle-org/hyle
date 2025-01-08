use anyhow::{bail, Result};
use sdk::{flatten_blobs, ContractInput, HyleOutput, Verifier};

use crate::ProofData;

pub async fn prove(
    binary: &[u8],
    contract_input: &ContractInput,
    verifier: &Verifier,
) -> Result<(ProofData, HyleOutput)> {
    match verifier.0.as_str() {
        "test" => {
            // FIXME: this is a hack to make the test pass
            let next_state = contract_input.initial_state.clone();
            let hyle_output = HyleOutput {
                version: 1,
                initial_state: contract_input.initial_state.clone(),
                next_state,
                identity: contract_input.identity.clone(),
                tx_hash: contract_input.tx_hash.clone(),
                index: contract_input.index.clone(),
                blobs: flatten_blobs(&contract_input.blobs),
                success: true,
                program_outputs: vec![],
            };
            Ok((
                ProofData::Bytes(serde_json::to_vec(&vec![&hyle_output])?),
                hyle_output,
            ))
        }
        "risc0" => {
            let contract_input = bonsai_runner::as_input_data(&contract_input)?;

            let explicit = std::env::var("RISC0_PROVER").unwrap_or_default();
            let receipt = match explicit.to_lowercase().as_str() {
                "bonsai" => bonsai_runner::run_bonsai(binary, contract_input.clone()).await?,
                _ => {
                    let env = risc0_zkvm::ExecutorEnv::builder()
                        .write_slice(&contract_input)
                        .build()
                        .unwrap();

                    let prover = risc0_zkvm::default_prover();
                    let prove_info = prover.prove(env, binary)?;
                    prove_info.receipt
                }
            };

            let hyle_output = receipt
                .journal
                .decode::<HyleOutput>()
                .expect("Failed to decode journal");

            if !hyle_output.success {
                let program_error = std::str::from_utf8(&hyle_output.program_outputs).unwrap();
                bail!(
                    "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
                    program_error
                );
            }

            let encoded_receipt = borsh::to_vec(&receipt).expect("Unable to encode receipt");
            Ok((ProofData::Bytes(encoded_receipt), hyle_output))
        }
        _ => bail!("Unsupported verifier: {}", verifier.0),
    }
}
