use anyhow::{bail, Result};
use client_sdk::transaction_builder::{BuildResult, StateUpdater, TransactionBuilder};
use hydentity::Hydentity;
use hyle::model::BlobTransaction;
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
            _ => bail!("Unknown contract name"),
        }
        Ok(())
    }

    fn get(&self, contract_name: &ContractName) -> Result<hyle_contract_sdk::StateDigest> {
        Ok(match contract_name.0.as_str() {
            "hyllar" => self.hyllar.as_digest(),
            "hydentity" => self.hydentity.as_digest(),
            _ => bail!("Unknown contract name"),
        })
    }
}

pub async fn generate(url: String) -> Result<()> {
    let indexer_client = IndexerApiHttpClient::new(url.clone());
    let client = NodeApiHttpClient::new(url);

    let hyllar = indexer_client.fetch_current_state(&"hyllar".into()).await?;
    let hydentity = indexer_client
        .fetch_current_state(&"hydentity".into())
        .await?;

    let mut states = States { hyllar, hydentity };

    let mut blob_txs = vec![];
    let ident = Identity("test.hydentity".to_string());

    let mut transaction = TransactionBuilder::new(ident.clone());

    let users = 20;

    for _ in 0..users {
        states
            .hydentity
            .default_builder(&mut transaction)
            .register_identity("password".to_string())?;

        let BuildResult {
            identity, blobs, ..
        } = transaction.build(&mut states).unwrap();

        blob_txs.push(BlobTransaction { identity, blobs });
    }

    // serialize to json and write to file
    std::fs::write("blob_txs.json", serde_json::to_string(&blob_txs).unwrap()).unwrap();
    Ok(())
}
pub async fn send(url: String) -> Result<()> {
    info!("Sending transactions");

    let blob_txs: Vec<BlobTransaction> =
        serde_json::from_str(&std::fs::read_to_string("blob_txs.json")?)?;

    // Spin out a few tasks to send the transactions in parallel
    let mut tasks = tokio::task::JoinSet::new();
    let par_tasks = 20;
    let repetitions = 1000;
    let txs_per_rep = blob_txs.len();
    for _ in 0..par_tasks {
        let blob_txs = blob_txs.clone();
        let url = url.clone();
        tasks.spawn(async move {
            let mut txs_sent = 0;
            let client = NodeApiHttpClient::new(url);
            for _ in 0..repetitions {
                for blob_tx in &blob_txs {
                    client.send_tx_blob(blob_tx).await.unwrap();
                    txs_sent += 1;
                }
            }
            info!("Transactions sent: {:?}", txs_sent);
        });
    }
    tasks.join_all().await;

    info!(
        "Transactions sent: {:?} total",
        txs_per_rep * repetitions * par_tasks
    );

    Ok(())
}
/*
let blob_tx_hash = client
.send_tx_blob(&)
.await
.unwrap();
*/
/*
for (proof, contract_name) in transaction.iter_prove() {
    let proof: ProofData = proof.await.unwrap();
    client
        .send_tx_proof(&hyle::model::ProofTransaction {
            tx_hashes: vec![blob_tx_hash.clone()],
            proof,
            contract_name,
        })
        .await
        .unwrap();
}
*/
/*
{
    let mut transaction = TransactionBuilder::new("faucet.hydentity".into());

    states
        .hydentity
        .default_builder(&mut transaction)
        .verify_identity(&states.hydentity, "password".to_string())?;
    states
        .hyllar
        .default_builder(&mut transaction)
        .transfer(node_identity.0.clone(), 100)?;

    send_transaction(ctx.client(), transaction, &mut states).await;
}
*/
