use std::pin::Pin;

use anyhow::Result;
use borsh::BorshSerialize;
use sdk::{
    Calldata, ContractName, HyleOutput, ProgramId, ProofData, RegisterContractAction,
    StateCommitment, TimeoutWindow, Verifier,
};

use crate::transaction_builder::ProvableBlobTx;

pub fn register_hyle_contract(
    builder: &mut ProvableBlobTx,
    new_contract_name: ContractName,
    verifier: Verifier,
    program_id: ProgramId,
    state_commitment: StateCommitment,
    timeout_window: Option<TimeoutWindow>,
    constructor_metadata: Option<Vec<u8>>,
) -> anyhow::Result<()> {
    builder.add_action(
        "hyle".into(),
        RegisterContractAction {
            contract_name: new_contract_name,
            verifier,
            program_id,
            state_commitment,
            timeout_window,
            constructor_metadata,
        },
        None,
        None,
        None,
    )?;
    Ok(())
}

pub trait ClientSdkProver<T: BorshSerialize + Send> {
    fn prove(
        &self,
        commitment_metadata: Vec<u8>,
        calldatas: T,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>>;
}

#[cfg(feature = "risc0")]
pub mod risc0 {

    use borsh::BorshSerialize;

    use super::*;

    pub struct Risc0Prover<'a> {
        binary: &'a [u8],
    }
    impl<'a> Risc0Prover<'a> {
        pub fn new(binary: &'a [u8]) -> Self {
            Self { binary }
        }
        pub async fn prove<T: BorshSerialize>(
            &self,
            commitment_metadata: Vec<u8>,
            calldatas: T,
        ) -> Result<ProofData> {
            let explicit = std::env::var("RISC0_PROVER").unwrap_or_default();
            let receipt = match explicit.to_lowercase().as_str() {
                "bonsai" => {
                    let input_data =
                        bonsai_runner::as_input_data(&(commitment_metadata, calldatas))?;
                    bonsai_runner::run_bonsai(self.binary, input_data.clone()).await?
                }
                "boundless" => {
                    let input_data =
                        bonsai_runner::as_input_data(&(commitment_metadata, calldatas))?;
                    bonsai_runner::run_boundless(self.binary, input_data).await?
                }
                _ => {
                    let input_data = borsh::to_vec(&(commitment_metadata, calldatas))?;
                    let env = risc0_zkvm::ExecutorEnv::builder()
                        .write(&input_data.len())?
                        .write_slice(&input_data)
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

    impl<T: BorshSerialize + Send + 'static> ClientSdkProver<T> for Risc0Prover<'_> {
        fn prove(
            &self,
            commitment_metadata: Vec<u8>,
            calldatas: T,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>> {
            Box::pin(self.prove(commitment_metadata, calldatas))
        }
    }
}

#[cfg(feature = "sp1")]
pub mod sp1 {
    use sp1_sdk::{
        network::builder::NetworkProverBuilder, EnvProver, NetworkProver, ProverClient,
        SP1ProvingKey, SP1Stdin,
    };

    use super::*;

    pub struct SP1Prover {
        pk: SP1ProvingKey,
        client: ProverType,
    }

    enum ProverType {
        Local(EnvProver),
        Network(Box<NetworkProver>),
    }

    impl SP1Prover {
        pub async fn new(pk: SP1ProvingKey) -> Self {
            let prover_type = std::env::var("SP1_PROVER").unwrap_or_default();

            match prover_type.to_lowercase().as_str() {
                "network" => {
                    // Setup the program for network proving
                    let client = NetworkProverBuilder::default().build();

                    tracing::info!("Registering sp1 program on network");
                    client
                        .register_program(&pk.vk, &pk.elf)
                        .await
                        .expect("registering program");

                    Self {
                        pk,
                        client: ProverType::Network(Box::new(client)),
                    }
                }
                _ => {
                    // Setup the program for local proving
                    let client = ProverClient::from_env();
                    Self {
                        pk,
                        client: ProverType::Local(client),
                    }
                }
            }
        }

        pub fn program_id(&self) -> Result<sdk::ProgramId> {
            Ok(sdk::ProgramId(serde_json::to_vec(&self.pk.vk)?))
        }

        pub async fn prove<T: BorshSerialize>(
            &self,
            commitment_metadata: Vec<u8>,
            calldatas: T,
        ) -> Result<ProofData> {
            // Setup the inputs.
            let mut stdin = SP1Stdin::new();
            let encoded = borsh::to_vec(&(commitment_metadata, calldatas))?;
            stdin.write_vec(encoded);

            // Generate the proof based on the prover type
            let proof = match &self.client {
                ProverType::Local(client) => client
                    .prove(&self.pk, &stdin)
                    .compressed()
                    .run()
                    .expect("failed to generate proof"),
                ProverType::Network(client) => client
                    .prove(&self.pk, &stdin)
                    // Core proofs are limite to 5M cycles
                    .compressed()
                    // Reserved strategy is better for higher usage throughput
                    .strategy(sp1_sdk::network::FulfillmentStrategy::Reserved)
                    .run()
                    .expect("failed to generate proof"),
            };

            let encoded_receipt = bincode::serialize(&proof)?;
            Ok(ProofData(encoded_receipt))
        }
    }

    impl<T: BorshSerialize + Send + 'static> ClientSdkProver<T> for SP1Prover {
        fn prove(
            &self,
            commitment_metadata: Vec<u8>,
            calldatas: T,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>> {
            Box::pin(self.prove(commitment_metadata, calldatas))
        }
    }
}

pub mod test {
    use crate::transaction_builder::TxExecutorHandler;

    use super::*;

    /// Generates valid proofs for the 'test' verifier using the TxExecutor
    pub struct TxExecutorTestProver<C: TxExecutorHandler> {
        contract: std::sync::Arc<std::sync::Mutex<C>>,
    }

    impl<C: TxExecutorHandler> TxExecutorTestProver<C> {
        pub fn new(contract: C) -> Self {
            Self {
                contract: std::sync::Arc::new(std::sync::Mutex::new(contract)),
            }
        }
    }

    impl<C: TxExecutorHandler> ClientSdkProver<Vec<Calldata>> for TxExecutorTestProver<C> {
        fn prove(
            &self,
            _commitment_metadata: Vec<u8>,
            calldatas: Vec<Calldata>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>>
        {
            let hos = calldatas
                .iter()
                .map(|calldata| self.contract.lock().unwrap().handle(calldata))
                .collect::<Result<Vec<_>>>();
            Box::pin(async move {
                match hos {
                    Ok(hos) => Ok(ProofData(borsh::to_vec(&hos).unwrap())),
                    Err(e) => Err(e),
                }
            })
        }
    }

    pub struct MockProver {}

    impl ClientSdkProver<Calldata> for MockProver {
        fn prove(
            &self,
            commitment_metadata: Vec<u8>,
            calldata: Calldata,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>> {
            Box::pin(async move {
                let hyle_output = execute(commitment_metadata.clone(), calldata.clone())?;
                Ok(ProofData(
                    borsh::to_vec(&hyle_output).expect("Failed to encode proof"),
                ))
            })
        }
    }

    impl ClientSdkProver<Vec<Calldata>> for MockProver {
        fn prove(
            &self,
            commitment_metadata: Vec<u8>,
            calldata: Vec<Calldata>,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<ProofData>> + Send + '_>> {
            Box::pin(async move {
                let mut proofs = Vec::new();
                for call in calldata {
                    let hyle_output = test::execute(commitment_metadata.clone(), call)?;
                    proofs.push(hyle_output);
                }
                Ok(ProofData(borsh::to_vec(&proofs)?))
            })
        }
    }

    pub fn execute(commitment_metadata: Vec<u8>, calldata: Calldata) -> Result<HyleOutput> {
        // FIXME: this is a hack to make the test pass.
        let initial_state = StateCommitment(commitment_metadata);
        let hyle_output = HyleOutput {
            version: 1,
            initial_state: initial_state.clone(),
            next_state: initial_state,
            identity: calldata.identity.clone(),
            index: calldata.index,
            blobs: calldata.blobs.clone(),
            tx_blob_count: calldata.tx_blob_count,
            success: true,
            tx_hash: calldata.tx_hash.clone(),
            state_reads: vec![],
            tx_ctx: None,
            onchain_effects: vec![],
            program_outputs: vec![],
        };
        Ok(hyle_output)
    }
}
