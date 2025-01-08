use anyhow::{bail, Result};
use client_sdk::transaction_builder::{BuildResult, StateUpdater, TransactionBuilder};
use client_sdk::ProofData;
use hydentity::Hydentity;
use hyle::model::{BlobTransaction, Hashable, ProofTransaction, RegisterContractTransaction};
use hyle::rest::client::NodeApiHttpClient;
use hyle_contract_sdk::erc20::ERC20;
use hyle_contract_sdk::Digestable;
use hyle_contract_sdk::{ContractName, Identity};
use hyllar::{HyllarToken, HyllarTokenContract};
use tokio::task::JoinSet;
use tracing::info;

#[derive(Debug, Clone)]
pub struct States {
    pub hyllar: HyllarToken,
    pub hydentity: Hydentity,
}

impl StateUpdater for States {
    fn update(
        &mut self,
        contract_name: &hyle_contract_sdk::ContractName,
        new_state: hyle_contract_sdk::StateDigest,
    ) -> Result<()> {
        match contract_name.0.as_str() {
            "hyllar-test" => self.hyllar = new_state.try_into()?,
            "hydentity" => self.hydentity = new_state.try_into()?,
            _ => bail!("Unknown contract name: {contract_name}"),
        }
        Ok(())
    }

    fn get(&self, contract_name: &ContractName) -> Result<hyle_contract_sdk::StateDigest> {
        Ok(match contract_name.0.as_str() {
            "hyllar-test" => self.hyllar.as_digest(),
            "hydentity" => self.hydentity.as_digest(),
            _ => bail!("Unknown contract name: {contract_name}"),
        })
    }
}

pub fn setup_hyllar(users: u32) -> Result<HyllarTokenContract> {
    let hyllar_token = HyllarToken::new(0, "faucet.hyllar-test".into());
    let mut hyllar_contract =
        HyllarTokenContract::init(hyllar_token.clone(), "faucet.hyllar-test".into());

    // Create an entry for each users
    for n in 0..users {
        let ident = &format!("{n}.hyllar-test");
        hyllar_contract
            .transfer(ident, 0)
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    Ok(hyllar_contract)
}

/// Create a new contract "hyllar-test" that already contains entries for each users
pub async fn setup(url: String, users: u32, verifier: String) -> Result<()> {
    let node_client = NodeApiHttpClient::new(url.clone());

    let hyllar_contract = setup_hyllar(users)?;

    let tx = RegisterContractTransaction {
        contract_name: "hyllar-test".into(),
        owner: "hyle".into(),
        verifier: verifier.into(),
        program_id: hyle_contracts::HYLLAR_ID.to_vec().into(),
        state_digest: hyllar_contract.state().as_digest(),
    };
    node_client.send_tx_register_contract(&tx).await?;

    Ok(())
}

pub async fn generate(users: u32, verifier: String, states: States) -> Result<()> {
    generate_blobs_txs(users, states.clone()).await?;
    generate_proof_txs(users, verifier, states).await?;

    Ok(())
}

pub async fn generate_blobs_txs(users: u32, states: States) -> Result<()> {
    let mut blob_txs = vec![];
    let mut tasks = JoinSet::new();
    let number_of_tasks = 100;
    let chunk_size: usize = users.div_ceil(number_of_tasks).try_into().unwrap();

    let user_chunks: Vec<_> = (0..users).collect();
    let user_chunks = user_chunks
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>();

    for chunk in user_chunks {
        let states = states.clone();

        tasks.spawn(async move {
            let mut local_blob_txs = vec![];

            for n in &chunk {
                info!(
                    "Building blob transaction for user: {n}/{:?}",
                    chunk.last().unwrap()
                );
                let ident = Identity(format!("{n}.hyllar-test").to_string());
                let mut transaction = TransactionBuilder::new(ident.clone());
                states
                    .hyllar
                    .builder("hyllar-test".into(), &mut transaction)
                    .transfer(ident.clone().to_string(), 0)?;

                let BuildResult {
                    identity, blobs, ..
                } = transaction.stateless_build()?;

                local_blob_txs.push(BlobTransaction { identity, blobs });
            }

            Ok::<_, anyhow::Error>(local_blob_txs)
        });
    }

    while let Some(result) = tasks.join_next().await {
        blob_txs.extend(result??);
    }

    std::fs::write("blob_txs.json", serde_json::to_string(&blob_txs).unwrap()).unwrap();
    Ok(())
}

pub async fn generate_proof_txs(users: u32, verifier: String, states: States) -> Result<()> {
    let mut proof_txs = vec![];
    let mut tasks = JoinSet::new();
    let number_of_tasks = 100;
    let chunk_size: usize = users.div_ceil(number_of_tasks).try_into().unwrap();

    let user_chunks: Vec<_> = (0..users).collect();
    let user_chunks = user_chunks
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>();
    for chunk in user_chunks {
        let mut states = states.clone();
        let verifier = verifier.clone();

        tasks.spawn(async move {
            let mut local_proof_txs = vec![];

            for n in &chunk {
                info!(
                    "Building proof transaction for user: {n}/{:?}",
                    chunk.last().unwrap()
                );
                let ident = Identity(format!("{n}.hyllar-test").to_string());
                let mut transaction = TransactionBuilder::new(ident.clone());
                states
                    .hyllar
                    .builder("hyllar-test".into(), &mut transaction)
                    .transfer(ident.clone().to_string(), 0)?;

                let BuildResult {
                    identity, blobs, ..
                } = transaction.build(&mut states)?;

                let blob_tx = BlobTransaction { identity, blobs };

                for (proof, contract_name) in transaction.iter_prove(&verifier.clone().into()) {
                    let proof: ProofData = proof.await.unwrap();
                    local_proof_txs.push(ProofTransaction {
                        contract_name,
                        proof,
                        tx_hashes: vec![blob_tx.hash()],
                    });
                }
            }

            Ok::<_, anyhow::Error>(local_proof_txs)
        });
    }

    while let Some(result) = tasks.join_next().await {
        proof_txs.extend(result??);
    }

    // serialize to json and write to file
    std::fs::write("proof_txs.json", serde_json::to_string(&proof_txs).unwrap()).unwrap();

    Ok(())
}

pub async fn send(url: String) -> Result<()> {
    send_blob_txs(url.clone()).await?;
    send_proof_txs(url).await?;
    Ok(())
}

pub async fn send_blob_txs(url: String) -> Result<()> {
    info!("Sending blob transactions");

    let blob_txs: Vec<BlobTransaction> =
        serde_json::from_str(&std::fs::read_to_string("blob_txs.json")?)?;

    // Spin out a few tasks to send the transactions in parallel
    let mut tasks = tokio::task::JoinSet::new();
    let number_of_tasks = 20;
    let chunk_size = blob_txs.len().div_ceil(number_of_tasks);
    for chunk in blob_txs.chunks(chunk_size) {
        let chunk = chunk.to_vec();
        let url = url.clone();
        tasks.spawn(async move {
            let client = NodeApiHttpClient::new(url);
            for blob_tx in chunk.iter() {
                client.send_tx_blob(blob_tx).await.unwrap();
            }
            info!("Blob transactions sent: {:?}", chunk.len());
        });
    }
    tasks.join_all().await;

    info!("Transactions sent: {:?} total", blob_txs.len());

    Ok(())
}

pub async fn send_proof_txs(url: String) -> Result<()> {
    info!("Sending proof transactions");

    let proof_txs: Vec<ProofTransaction> =
        serde_json::from_str(&std::fs::read_to_string("proof_txs.json")?)?;

    // Spin out a few tasks to send the transactions in parallel
    let mut tasks = tokio::task::JoinSet::new();
    let number_of_tasks = 20;
    let chunk_size = proof_txs.len().div_ceil(number_of_tasks);
    for chunk in proof_txs.chunks(chunk_size) {
        let chunk = chunk.to_vec();
        let url = url.clone();
        tasks.spawn(async move {
            let client = NodeApiHttpClient::new(url);
            for proof_tx in chunk.iter() {
                client.send_tx_proof(proof_tx).await.unwrap();
            }
            info!("Transactions sent: {:?}", chunk.len());
        });
    }
    tasks.join_all().await;

    info!("Proof transactions sent: {:?} total", proof_txs.len());

    Ok(())
}
