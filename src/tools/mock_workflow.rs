use crate::{
    bus::SharedMessageBus,
    handle_messages,
    model::{Blob, BlobData, BlobTransaction, ContractName, Identity, Transaction},
    p2p::network::MempoolNetMessage,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunScenario {
    StressTest,
}

pub struct MockWorkflowHandler {
    bus: SharedMessageBus,
}

impl MockWorkflowHandler {
    pub fn new(bus: SharedMessageBus) -> MockWorkflowHandler {
        MockWorkflowHandler { bus }
    }

    pub async fn start(&mut self) {
        handle_messages! {
            listen<RunScenario>(self.bus) = cmd => {
                match cmd {
                    RunScenario::StressTest => {
                        self.stress_test().await;
                    }
                }
            },
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
