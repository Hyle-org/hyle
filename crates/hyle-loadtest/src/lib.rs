use anyhow::{bail, Result};
use client_sdk::transaction_builder::{BuildResult, StateUpdater, TransactionBuilder};
use client_sdk::ProofData;
use hydentity::Hydentity;
use hyle::model::{BlobTransaction, Hashable, ProofTransaction};
use hyle::rest::client::{IndexerApiHttpClient, NodeApiHttpClient};
use hyle_contract_sdk::Digestable;
use hyle_contract_sdk::{ContractName, Identity};
use hyllar::HyllarToken;
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
            "hyllar" => self.hyllar = new_state.try_into()?,
            "hydentity" => self.hydentity = new_state.try_into()?,
            _ => bail!("Unknown contract name: {contract_name}"),
        }
        Ok(())
    }

    fn get(&self, contract_name: &ContractName) -> Result<hyle_contract_sdk::StateDigest> {
        Ok(match contract_name.0.as_str() {
            "hyllar" => self.hyllar.as_digest(),
            "hydentity" => self.hydentity.as_digest(),
            _ => bail!("Unknown contract name {contract_name}"),
        })
    }
}

/// Setup hyllar contract by sending a tx to "test" from faucet, in order to create an entry for that user.
pub async fn setup(url: String) -> Result<()> {
    let node_client = NodeApiHttpClient::new(url.clone());
    let indexer_client = IndexerApiHttpClient::new(url.clone());

    let hyllar = indexer_client.fetch_current_state(&"hyllar".into()).await?;
    let hydentity = indexer_client
        .fetch_current_state(&"hydentity".into())
        .await?;

    let mut states = States { hyllar, hydentity };
    let identity = Identity("faucet.hydentity".to_string());
    let mut transaction = TransactionBuilder::new(identity.clone());

    // Verify faucet identity
    states
        .hydentity
        .default_builder(&mut transaction)
        .verify_identity(&states.hydentity, "password".to_string())?;

    // Transfer to test
    states
        .hyllar
        .default_builder(&mut transaction)
        .transfer("test.hyllar".to_string(), 0)?;

    let BuildResult {
        identity, blobs, ..
    } = transaction.build(&mut states).unwrap();

    let blob_tx = BlobTransaction { identity, blobs };
    node_client.send_tx_blob(&blob_tx).await.unwrap();

    for (proof, contract_name) in transaction.iter_prove() {
        let proof: ProofData = proof.await.unwrap();
        let proof_tx = ProofTransaction {
            contract_name,
            proof,
            tx_hashes: vec![blob_tx.hash()],
        };
        node_client.send_tx_proof(&proof_tx).await.unwrap();
    }

    Ok(())
}

pub async fn generate(url: String, users: u32) -> Result<()> {
    let indexer_client = IndexerApiHttpClient::new(url.clone());

    let hyllar = indexer_client.fetch_current_state(&"hyllar".into()).await?;
    let hydentity = indexer_client
        .fetch_current_state(&"hydentity".into())
        .await?;

    let mut states = States { hyllar, hydentity };

    let mut blob_txs = vec![];
    let mut proof_txs = vec![];

    let ident = Identity("test.hyllar".to_string());

    let mut transaction = TransactionBuilder::new(ident.clone());
    states
        .hyllar
        .default_builder(&mut transaction)
        .transfer(ident.clone().to_string(), 0)?;

    let BuildResult {
        identity, blobs, ..
    } = transaction.build(&mut states).unwrap();

    let blob_tx = BlobTransaction { identity, blobs };
    blob_txs.push(blob_tx.clone());

    for (proof, contract_name) in transaction.iter_prove() {
        let proof: ProofData = proof.await.unwrap();
        proof_txs.push(ProofTransaction {
            contract_name,
            proof,
            tx_hashes: vec![blob_tx.hash()],
        });
    }

    // We now have 1 blobTx and 1 proofTx. We want to duplicate it for each user
    // This is a hacky. Correct behaviour should be to iterate over users when creating the transactions.
    for _ in 0..users - 1 {
        blob_txs.push(blob_tx.clone());
        proof_txs.extend(proof_txs.first().cloned());
    }

    // serialize to json and write to file
    std::fs::write("blob_txs.json", serde_json::to_string(&blob_txs).unwrap()).unwrap();
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
