use crate::{
    bus::{BusClient, SharedMessageBus},
    handle_messages,
    mempool::MempoolNetMessage,
    model::{Blob, BlobData, BlobTransaction, ContractName, Identity, Transaction},
};
use frunk::{HCons, HNil};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::{Receiver, Sender};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunScenario {
    StressTest,
}

pub struct MockWorkflowHandler {
    bus: BusClient<HCons<Receiver<RunScenario>, HCons<Sender<MempoolNetMessage>, HNil>>>,
}

impl MockWorkflowHandler {
    pub async fn new(bus: SharedMessageBus) -> MockWorkflowHandler {
        MockWorkflowHandler {
            bus: BusClient::new()
                .with_sender::<MempoolNetMessage>(&bus)
                .await
                .with_receiver::<RunScenario>(&bus)
                .await,
        }
    }

    pub async fn start(&mut self) {
        handle_messages! {
            on_bus self.bus,
            listen_client<RunScenario> cmd => {
                match cmd {
                    RunScenario::StressTest => {
                        self.stress_test().await;
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
}
