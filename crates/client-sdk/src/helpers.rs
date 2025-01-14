use crate::ProofData;
use anyhow::{bail, Result};
use sdk::{flatten_blobs, ContractInput, HyleOutput};

pub enum Prover {
    #[cfg(feature = "risc0")]
    Risc0Prover,
    #[cfg(feature = "sp1")]
    SP1Prover,
    TestProver,
}

impl Prover {
    pub async fn prove(
        &self,
        binary: &[u8],
        contract_input: &ContractInput,
    ) -> Result<(ProofData, HyleOutput)> {
        match self {
            #[cfg(feature = "risc0")]
            Prover::Risc0Prover => risc0::prove(binary, contract_input).await,
            #[cfg(feature = "sp1")]
            Prover::SP1Prover => sp1::prove(binary, contract_input),
            Prover::TestProver => test::prove(binary, contract_input),
        }
    }

    pub fn execute(&self, binary: &[u8], contract_input: &ContractInput) -> Result<HyleOutput> {
        match self {
            #[cfg(feature = "risc0")]
            Prover::Risc0Prover => risc0::execute(binary, contract_input),
            #[cfg(feature = "sp1")]
            Prover::SP1Prover => sp1::execute(binary, contract_input),
            Prover::TestProver => test::execute(binary, contract_input),
        }
    }
}

#[cfg(feature = "risc0")]
pub mod risc0 {
    use super::*;

    pub fn execute(binary: &[u8], contract_input: &ContractInput) -> Result<HyleOutput> {
        let contract_input = bonsai_runner::as_input_data(contract_input)?;
        let env = risc0_zkvm::ExecutorEnv::builder()
            .write_slice(&contract_input)
            .build()
            .unwrap();

        let executor = risc0_zkvm::default_executor();
        let execute_info = executor.execute(env, binary)?;
        let output = execute_info
            .journal
            .decode::<HyleOutput>()
            .expect("Failed to decode journal");

        check_output(&output)?;

        Ok(output)
    }

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

        let output = receipt
            .journal
            .decode::<HyleOutput>()
            .expect("Failed to decode journal");

        check_output(&output)?;

        let encoded_receipt = borsh::to_vec(&receipt).expect("Unable to encode receipt");
        Ok((ProofData::Bytes(encoded_receipt), output))
    }
}

#[cfg(feature = "sp1")]
pub mod sp1 {
    use anyhow::Context;
    use sp1_sdk::{ProverClient, SP1Stdin};

    use super::*;

    pub fn execute(binary: &[u8], contract_input: &ContractInput) -> Result<HyleOutput> {
        let client = ProverClient::from_env();
        let mut stdin = SP1Stdin::new();
        stdin.write(&contract_input);

        let (public_values, _) = client
            .execute(binary, &stdin)
            .run()
            .expect("failed to generate proof");

        let (hyle_output, _) = bincode::decode_from_slice::<HyleOutput, _>(
            public_values.as_slice(),
            bincode::config::legacy().with_fixed_int_encoding(),
        )
        .context("Failed to extract HyleOuput from SP1 proof")?;

        check_output(&hyle_output)?;

        Ok(hyle_output)
    }

    pub fn prove(
        binary: &[u8],
        contract_input: &ContractInput,
    ) -> anyhow::Result<(ProofData, HyleOutput)> {
        let client = ProverClient::from_env();

        // Setup the inputs.
        let mut stdin = SP1Stdin::new();
        stdin.write(&contract_input);

        // Setup the program for proving.
        let (pk, _vk) = client.setup(binary);

        // Generate the proof
        let proof = client
            .prove(&pk, &stdin)
            //.compressed()
            .run()
            .expect("failed to generate proof");

        let (hyle_output, _) = bincode::decode_from_slice::<HyleOutput, _>(
            proof.public_values.as_slice(),
            bincode::config::legacy().with_fixed_int_encoding(),
        )
        .context("Failed to extract HyleOuput from SP1 proof")?;

        check_output(&hyle_output)?;

        let encoded_receipt = bincode::serde::encode_to_vec(
            &proof,
            bincode::config::legacy().with_fixed_int_encoding(),
        )?;
        Ok((ProofData::Bytes(encoded_receipt), hyle_output))
    }
}

pub mod test {
    use super::*;

    pub fn execute(_binary: &[u8], contract_input: &ContractInput) -> Result<HyleOutput> {
        // FIXME: this is a hack to make the test pass.
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
        Ok(hyle_output)
    }

    pub fn prove(
        binary: &[u8],
        contract_input: &ContractInput,
    ) -> anyhow::Result<(ProofData, HyleOutput)> {
        let hyle_output = test::execute(binary, contract_input)?;

        check_output(&hyle_output)?;
        Ok((
            ProofData::Bytes(bincode::encode_to_vec(
                vec![hyle_output.clone()],
                bincode::config::standard(),
            )?),
            hyle_output,
        ))
    }
}

fn check_output(output: &HyleOutput) -> Result<()> {
    if !output.success {
        let program_error = std::str::from_utf8(&output.program_outputs).unwrap();
        bail!(
            "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
            program_error
        );
    }
    Ok(())
}
