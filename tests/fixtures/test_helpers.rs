use client_sdk::transaction_builder::{ProvableBlobTx, StateUpdater, TxExecutor};

use hyle::{
    model::{BlobTransaction, ProofData},
    rest::client::NodeApiHttpClient,
};
use std::time::Duration;
use tokio::time::timeout;
use tracing::info;

pub use hyle_test::conf_maker::ConfMaker;
pub use hyle_test::node_process::TestProcess;

pub async fn wait_height(client: &NodeApiHttpClient, heights: u64) -> anyhow::Result<()> {
    wait_height_timeout(client, heights, 30).await
}

pub async fn wait_height_timeout(
    client: &NodeApiHttpClient,
    heights: u64,
    timeout_duration: u64,
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(timeout_duration), async {
        loop {
            if let Ok(mut current_height) = client.get_block_height().await {
                let target_height = current_height + heights;
                while current_height.0 < target_height.0 {
                    info!(
                        "⏰ Waiting for height {} to be reached. Current is {}",
                        target_height, current_height
                    );
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    current_height = client.get_block_height().await?;
                }
                return anyhow::Ok(());
            } else {
                info!("⏰ Waiting for node to be ready");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("Timeout reached while waiting for height: {e}"))?
}

#[allow(dead_code)]
pub async fn send_transaction<S: StateUpdater>(
    client: &NodeApiHttpClient,
    transaction: ProvableBlobTx,
    ctx: &mut TxExecutor<S>,
) {
    let identity = transaction.identity.clone();
    let blobs = transaction.blobs.clone();
    client
        .send_tx_blob(&BlobTransaction { identity, blobs })
        .await
        .unwrap();

    let provable_tx = ctx.process(transaction).unwrap();
    for (proof, contract_name) in provable_tx.iter_prove() {
        let proof: ProofData = proof.await.unwrap();
        client
            .send_tx_proof(&hyle::model::ProofTransaction {
                proof,
                contract_name,
            })
            .await
            .unwrap();
    }
}
