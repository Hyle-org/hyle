use std::pin::Pin;

use anyhow::Result;
use sdk::{
    flatten_blobs, ContractInput, ContractName, HyleOutput, ProgramId, ProofData,
    RegisterContractAction, StateCommitment, Verifier,
};

use crate::transaction_builder::ProvableBlobTx;

pub fn register_hyle_contract(
    builder: &mut ProvableBlobTx,
    new_contract_name: ContractName,
    verifier: Verifier,
    program_id: ProgramId,
    state_commitment: StateCommitment,
) -> anyhow::Result<()> {
    builder.add_action(
        "hyle".into(),
        RegisterContractAction {
            contract_name: new_contract_name,
            verifier,
            program_id,
            state_commitment: state_commitment,
        },
        None,
        None,
        None,
    )?;
    Ok(())
}

pub trait ClientSdkProver {
    fn prove(
        &self,
        contract_input: ContractInput,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>>;
}

#[cfg(feature = "risc0")]
pub mod risc0 {
    use sdk::ContractInput;

    use super::*;

    pub struct Risc0Prover<'a> {
        binary: &'a [u8],
    }
    impl<'a> Risc0Prover<'a> {
        pub fn new(binary: &'a [u8]) -> Self {
            Self { binary }
        }
        pub async fn prove(&self, contract_input: ContractInput) -> Result<ProofData> {
            let explicit = std::env::var("RISC0_PROVER").unwrap_or_default();
            let receipt = match explicit.to_lowercase().as_str() {
                "bonsai" => {
                    let contract_input = bonsai_runner::as_input_data(&contract_input)?;
                    bonsai_runner::run_bonsai(self.binary, contract_input.clone()).await?
                }
                _ => {
                    let contract_input = borsh::to_vec(&contract_input)?;
                    let env = risc0_zkvm::ExecutorEnv::builder()
                        .write(&contract_input.len())?
                        .write_slice(&contract_input)
                        .build()
                        .unwrap();

                    let prover = risc0_zkvm::default_prover();
                    let prove_info = prover.prove(env, self.binary)?;
                    prove_info.receipt
                }
            };

            let encoded_receipt = borsh::to_vec(&receipt).expect("Unable to encode receipt");
            Ok(ProofData(encoded_receipt))
        }
    }

    impl ClientSdkProver for Risc0Prover<'_> {
        fn prove(
            &self,
            contract_input: ContractInput,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>> {
            Box::pin(self.prove(contract_input))
        }
    }
}

#[cfg(feature = "sp1")]
pub mod sp1 {
    use anyhow::Context;
    use sp1_sdk::{EnvProver, ProverClient, SP1ProvingKey, SP1Stdin, SP1VerifyingKey};

    use super::*;

    pub struct SP1Prover {
        pk: SP1ProvingKey,
        pub vk: SP1VerifyingKey,
        client: EnvProver,
    }
    impl SP1Prover {
        pub fn new(binary: &[u8]) -> Self {
            // Setup the program for proving.
            let client = ProverClient::from_env();
            let (pk, vk) = client.setup(binary);
            Self { client, pk, vk }
        }
        pub async fn prove(&self, contract_input: ContractInput) -> Result<ProofData> {
            // Setup the inputs.
            let mut stdin = SP1Stdin::new();
            let encoded = borsh::to_vec(&contract_input)?;
            stdin.write_vec(encoded);

            // Generate the proof
            let proof = self
                .client
                .prove(&self.pk, &stdin)
                //.compressed()
                .run()
                .expect("failed to generate proof");

            let hyle_output = borsh::from_slice::<HyleOutput>(proof.public_values.as_slice())
                .context("Failed to extract HyleOuput from SP1 proof")?;

            check_output(&hyle_output)?;

            let encoded_receipt = bincode::serialize(&proof)?;
            Ok(ProofData(encoded_receipt))
        }
    }

    impl ClientSdkProver for SP1Prover {
        fn prove(
            &self,
            contract_input: ContractInput,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>> {
            Box::pin(self.prove(contract_input))
        }
    }
}

pub mod test {
    use super::*;

    pub struct TestProver {}

    impl ClientSdkProver for TestProver {
        fn prove(
            &self,
            contract_input: ContractInput,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>> {
            Box::pin(async move {
                let hyle_output = test::execute(&contract_input)?;
                Ok(ProofData(borsh::to_vec(&vec![hyle_output])?))
            })
        }
    }

    pub fn execute(contract_input: &ContractInput) -> Result<HyleOutput> {
        // FIXME: this is a hack to make the test pass.
        let initial_state = StateCommitment(contract_input.state.clone());
        let hyle_output = HyleOutput {
            version: 1,
            initial_state: initial_state.clone(),
            next_state: initial_state,
            identity: contract_input.identity.clone(),
            index: contract_input.index,
            blobs: flatten_blobs(&contract_input.blobs),
            success: true,
            tx_hash: contract_input.tx_hash.clone(),
            tx_ctx: None,
            onchain_effects: vec![],
            program_outputs: vec![],
        };
        Ok(hyle_output)
    }
}

#[cfg(feature = "sp1")]
fn check_output(output: &HyleOutput) -> Result<()> {
    if !output.success {
        let program_error = std::str::from_utf8(&output.program_outputs).unwrap();
        anyhow::bail!(
            "\x1b[91mExecution failed ! Program output: {}\x1b[0m",
            program_error
        );
    }
    Ok(())
}
