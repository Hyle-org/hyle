use crate::{
    bus::{BusMessage, SharedMessageBus},
    handle_messages,
    mempool::MempoolNetMessage,
    model::{Blob, BlobData, BlobTransaction, ContractName, Identity, Transaction},
    utils::modules::Module,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunScenario {
    StressTest,
}
impl BusMessage for RunScenario {}

pub struct MockWorkflowHandler {
    bus: SharedMessageBus,
}

impl Module for MockWorkflowHandler {
    fn name() -> &'static str {
        "MockWorkflowHandler"
    }
    fn dependencies() -> Vec<&'static str> {
        vec![]
    }
}

impl MockWorkflowHandler {
    pub fn new(bus: SharedMessageBus) -> MockWorkflowHandler {
        MockWorkflowHandler { bus }
    }

    pub async fn start(&mut self) {
        handle_messages! {
            on_bus self.bus,
            listen<RunScenario> cmd => {
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
        let message_sender = self.bus.sender().await;
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
            let _ = message_sender.send(tx.clone());
        }
    }
}
