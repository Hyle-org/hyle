use std::sync::Arc;

use client_sdk::{rest_client::NodeApiHttpClient, transaction_builder::ProofTxBuilder};
use tokio::sync::{mpsc, Mutex};
use tracing::error;

pub struct Prover {
    sender: mpsc::UnboundedSender<ProofTxBuilder>,
}

impl Prover {
    pub fn new(node_client: Arc<NodeApiHttpClient>) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel::<ProofTxBuilder>();
        let receiver = Arc::new(Mutex::new(receiver));

        tokio::spawn(async move {
            while let Some(tx) = receiver.lock().await.recv().await {
                for proof in tx.iter_prove() {
                    match proof.await {
                        Ok(proof) => {
                            node_client.send_tx_proof(&proof).await.unwrap();
                        }
                        Err(e) => {
                            error!("failed to prove transaction: {e}");
                            continue;
                        }
                    };
                }
            }
        });

        Prover { sender }
    }

    pub async fn add(&self, tx: ProofTxBuilder) {
        if let Err(e) = self.sender.send(tx) {
            eprintln!("Failed to add transaction: {}", e);
        }
    }
}
