use crate::{
    bus::{BusMessage, SharedMessageBus},
    handle_messages,
    mempool::MempoolNetMessage,
    model::{
        Blob, BlobData, BlobTransaction, ContractName, Identity, SharedRunContext, Transaction,
    },
    utils::modules::Module,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::bus::bus_client;

bus_client! {
struct BusClient {
    sender(MempoolNetMessage),
    receiver(RunScenario),
}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunScenario {
    StressTest,
}
impl BusMessage for RunScenario {}

pub struct MockWorkflowHandler {
    bus: BusClient,
}

impl Module for MockWorkflowHandler {
    fn name() -> &'static str {
        "MockWorkflowHandler"
    }

    type Context = SharedRunContext;
    type Store = ();

    async fn build(ctx: &Self::Context) -> Result<Self> {
        Ok(Self::new(ctx.bus.new_handle()).await)
    }

    fn run(&mut self, _ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl MockWorkflowHandler {
    pub async fn new(bus: SharedMessageBus) -> MockWorkflowHandler {
        MockWorkflowHandler {
            bus: BusClient::new_from_bus(bus).await,
        }
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
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
