use anyhow::{Context, Result};
use reqwest::Url;

use crate::model::{BlobTransaction, ProofTransaction, RegisterContractTransaction};

pub struct ApiHttpClient {
    pub url: Url,
    pub reqwest_client: reqwest::Client,
}

impl ApiHttpClient {
    pub async fn send_tx_blob(&self, tx: &BlobTransaction) -> Result<String> {
        let res = self
            .reqwest_client
            .post(format!("{}v1/tx/send/blob", self.url))
            .body(serde_json::to_string(tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx blob")?;

        res.text().await.context("Decoding response")
    }
    pub async fn send_tx_proof(&self, tx: &ProofTransaction) -> Result<String> {
        let res = self
            .reqwest_client
            .post(format!("{}v1/tx/send/proof", self.url))
            .body(serde_json::to_string(&tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx proof")?;

        res.text().await.context("Decoding response")
    }
    pub async fn send_tx_register_contract(
        &self,
        tx: &RegisterContractTransaction,
    ) -> Result<String> {
        let res = self
            .reqwest_client
            .post(format!("{}v1/contract/register", self.url))
            .body(serde_json::to_string(&tx)?)
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Sending tx register contract")?;

        res.text().await.context("Decoding response")
    }
}
