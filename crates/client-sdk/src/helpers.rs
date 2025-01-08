#[cfg(feature = "risc0")]
pub mod risc0 {
    use crate::ProofData;
    use anyhow::{bail, Result};
    use sdk::{ContractInput, HyleOutput};

    pub async fn prove(
        binary: &[u8],
        contract_input: &ContractInput,
    ) -> Result<(ProofData, HyleOutput)> {
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
}

#[cfg(feature = "sp1")]
pub mod sp1 {
    use anyhow::Context;
    use sp1_sdk::{ProverClient, SP1Stdin};

    use crate::ProofData;
    use anyhow::{bail, Result};
    use sdk::{ContractInput, HyleOutput};

    pub async fn prove(
        binary: &[u8],
        contract_input: &ContractInput,
    ) -> anyhow::Result<(ProofData, HyleOutput)> {
        let client = ProverClient::new();

        // Setup the inputs.
        let mut stdin = SP1Stdin::new();
        stdin.write(&contract_input);

        // Setup the program for proving.
        let (pk, _vk) = client.setup(binary);

        // Generate the proof
        let proof = client
            .prove(&pk, stdin)
            //.compressed()
            .run()
            .expect("failed to generate proof");

        let (hyle_output, _) = bincode::decode_from_slice::<HyleOutput, _>(
            proof.public_values.as_slice(),
            bincode::config::legacy().with_fixed_int_encoding(),
        )
        .context("Failed to extract HyleOuput from SP1 proof")?;

        if !hyle_output.success {
            let program_error = std::str::from_utf8(&hyle_output.program_outputs).unwrap();
            anyhow::bail!(
                "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
                program_error
            );
        }

        let encoded_receipt = bincode::serde::encode_to_vec(
            &proof,
            bincode::config::legacy().with_fixed_int_encoding(),
        )?;
        Ok((ProofData::Bytes(encoded_receipt), hyle_output))
    }
}
