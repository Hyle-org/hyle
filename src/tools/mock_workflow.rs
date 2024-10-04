use std::time::Duration;

use crate::{
    bus::{BusMessage, SharedMessageBus},
    handle_messages,
    mempool::MempoolNetMessage,
    model::{
        Blob, BlobData, BlobTransaction, ContractName, Identity, ProofTransaction,
        RegisterContractTransaction, SharedRunContext, StateDigest, Transaction,
    },
    rest::client::ApiHttpClient,
    utils::modules::Module,
};
use anyhow::Result;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::bus::bus_client;

bus_client! {
struct MockWorkflowBusClient {
    sender(MempoolNetMessage),
    receiver(RunScenario),
}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunScenario {
    StressTest,
    ApiTest,
}
impl BusMessage for RunScenario {}

pub struct MockWorkflowHandler {
    bus: MockWorkflowBusClient,
}

impl Module for MockWorkflowHandler {
    fn name() -> &'static str {
        "MockWorkflowHandler"
    }

    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = MockWorkflowBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        Ok(MockWorkflowHandler { bus })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl MockWorkflowHandler {
    pub async fn start(&mut self) -> anyhow::Result<()> {
        handle_messages! {
            on_bus self.bus,
            listen<RunScenario> cmd => {
                match cmd {
                    RunScenario::StressTest => {
                        self.stress_test().await;
                    },
                    RunScenario::ApiTest => {
                        self.api_test().await;
                    }
                }
            }
        }
    }

    async fn stress_test(&mut self) {
        warn!("Starting stress test");
        let tx = MempoolNetMessage::NewTx(Transaction {
            version: 1,
            transaction_data: crate::model::TransactionData::Blob(BlobTransaction {
                identity: Identity("toto".to_string()),
                blobs: vec![Blob {
                    contract_name: ContractName("test".to_string()),
                    data: BlobData(vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
                }],
            }),
            inner: "???".to_string(),
        });
        for _ in 0..500000 {
            let _ = self.bus.send(tx.clone());
        }
    }

    async fn api_test(&mut self) {
        info!("Starting api test");

        let api_client = ApiHttpClient {
            url: Url::parse("http://localhost:4321").unwrap(),
            reqwest_client: Client::new(),
        };

        let tx_blob = BlobTransaction {
            identity: Identity("id".to_string()),
            blobs: vec![Blob {
                contract_name: ContractName("contract_name".to_string()),
                data: BlobData(vec![0, 1, 2]),
            }],
        };

        let tx_proof = ProofTransaction {
            blobs_references: vec![],
            proof: vec![],
        };

        let tx_contract = RegisterContractTransaction {
            owner: "owner".to_string(),
            verifier: "verifier".to_string(),
            program_id: vec![],
            state_digest: StateDigest(vec![]),
            contract_name: ContractName("contract".to_string()),
        };

        let mut i = 0;
        loop {
            i += 1;
            match (i % 3) + 1 {
                1 => {
                    info!("Sending tx blob");
                    let mut new_tx_blob = tx_blob.clone();
                    new_tx_blob.identity = Identity(format!("{}{}", tx_blob.identity.0, i));
                    _ = api_client.send_tx_blob(&new_tx_blob).await;
                }
                2 => {
                    info!("Sending tx proof");
                    let mut new_tx_proof = tx_proof.clone();
                    new_tx_proof.proof = vec![i];
                    _ = api_client.send_tx_proof(&tx_proof).await;
                }
                3 => {
                    info!("Sending contract");
                    let mut new_tx_contract = tx_contract.clone();
                    new_tx_contract.verifier = i.to_string();
                    _ = api_client.send_tx_register_contract(&tx_contract).await;
                }
                _ => {
                    error!("unknown random choice");
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    }
}
