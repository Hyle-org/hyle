use std::time::Duration;

use crate::{
    bus::{BusClientSender, BusMessage},
    mempool::api::RestApiMessage,
    model::{
        utils::get_current_timestamp, Blob, BlobData, BlobTransaction, ContractName, ProofData,
        ProofTransaction, RegisterContractTransaction, SharedRunContext, Transaction,
    },
    module_handle_messages,
    rest::client::NodeApiHttpClient,
    utils::modules::{module_bus_client, Module},
};
use anyhow::Result;
use hyle_contract_sdk::{Identity, ProgramId, StateDigest};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{error, info, warn};

// use crate::bus::bus_client;

module_bus_client! {
struct MockWorkflowBusClient {
    sender(RestApiMessage),
    receiver(RunScenario),
}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunScenario {
    StressTest,
    ApiTest {
        qps: u64,
        injection_duration_seconds: u64,
    },
}
impl BusMessage for RunScenario {}

pub struct MockWorkflowHandler {
    bus: MockWorkflowBusClient,
}

impl Module for MockWorkflowHandler {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = MockWorkflowBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        let api = api::api(&ctx.common).await;
        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/tools", api));
            }
        }

        Ok(MockWorkflowHandler { bus })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

mod api {
    use axum::{routing::post, Router};

    use crate::bus::metrics::BusMetrics;
    use crate::bus::BusClientSender;
    use crate::tools::mock_workflow::RunScenario;
    use crate::{bus::bus_client, model::CommonRunContext};
    use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

    bus_client! {
    struct RestBusClient {
        sender(RunScenario),
    }
    }
    pub struct RouterState {
        bus: RestBusClient,
    }

    pub(super) async fn api(ctx: &CommonRunContext) -> Router<()> {
        let state = RouterState {
            bus: RestBusClient::new_from_bus(ctx.bus.new_handle()).await,
        };

        Router::new()
            .route("/run_scenario", post(run_scenario))
            .with_state(state)
    }

    pub async fn run_scenario(
        State(mut state): State<RouterState>,
        Json(scenario): Json<RunScenario>,
    ) -> Result<impl IntoResponse, StatusCode> {
        state
            .bus
            .send(scenario)
            .map(|_| StatusCode::OK)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }

    impl Clone for RouterState {
        fn clone(&self) -> Self {
            use crate::utils::static_type_map::Pick;
            Self {
                bus: RestBusClient::new(
                    Pick::<BusMetrics>::get(&self.bus).clone(),
                    Pick::<tokio::sync::broadcast::Sender<RunScenario>>::get(&self.bus).clone(),
                ),
            }
        }
    }
}

impl MockWorkflowHandler {
    pub async fn start(&mut self) -> anyhow::Result<()> {
        module_handle_messages! {
            on_bus self.bus,
            listen<RunScenario> cmd => {
                match cmd {
                    RunScenario::StressTest => {
                        self.stress_test().await;
                    },
                    RunScenario::ApiTest { qps, injection_duration_seconds } => {
                        self.api_test(qps, injection_duration_seconds).await?;
                    }
                }
            }
        }

        Ok(())
    }

    async fn stress_test(&mut self) {
        warn!("Starting stress test");
        let tx = RestApiMessage::NewTx(Transaction {
            version: 1,
            transaction_data: crate::model::TransactionData::Blob(BlobTransaction {
                identity: Identity::new("toto"),
                blobs: vec![Blob {
                    contract_name: ContractName::new("test"),
                    data: BlobData(vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
                }],
            }),
        });
        for _ in 0..500000 {
            let _ = self.bus.send(tx.clone());
        }
    }

    async fn api_test(&mut self, qps: u64, injection_duration_seconds: u64) -> Result<()> {
        info!("Starting api test");

        let api_client = NodeApiHttpClient::new("http://localhost:4321".to_string())?;

        let tx_blob = BlobTransaction {
            identity: Identity::new("id"),
            blobs: vec![Blob {
                contract_name: ContractName::new("contract_name"),
                data: BlobData(vec![0, 1, 2]),
            }],
        };

        let tx_proof = ProofTransaction::default();

        let tx_contract = RegisterContractTransaction {
            owner: "owner".to_string(),
            verifier: "verifier".into(),
            program_id: ProgramId(vec![]),
            state_digest: StateDigest(vec![]),
            contract_name: ContractName::new("contract"),
        };

        let millis_interval = 1000_u64.div_ceil(qps);

        let injection_stop_date = get_current_timestamp() + injection_duration_seconds;

        let mut i = 0;
        loop {
            if get_current_timestamp() > injection_stop_date {
                info!("Stopped injection");
                break Ok(());
            }
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
                    new_tx_proof.proof = ProofData(vec![i]);
                    _ = api_client.send_tx_proof(&tx_proof).await;
                }
                3 => {
                    info!("Sending contract");
                    let mut new_tx_contract = tx_contract.clone();
                    new_tx_contract.verifier = i.to_string().into();
                    new_tx_contract.contract_name =
                        ContractName(format!("{}-{}", new_tx_contract.contract_name.0, i));
                    _ = api_client.send_tx_register_contract(&tx_contract).await;
                }
                _ => {
                    error!("unknown random choice");
                }
            }

            sleep(Duration::from_millis(millis_interval)).await;
        }
    }
}
