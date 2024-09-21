use crate::model::Transaction;
use crate::tools::mock_workflow::RunScenario;
use crate::{
    bus::command_response::CmdRespClient,
    model::{
        BlobTransaction, ContractName, Hashable, ProofTransaction, RegisterContractTransaction,
        TransactionData, TxHash,
    },
    node_state::{NodeStateQuery, NodeStateQueryResponse},
};
use anyhow::anyhow;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use tracing::info;

use super::{AppError, RouterState};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum RestApiMessage {
    NewTx(Transaction),
}

async fn handle_send(
    state: RouterState,
    payload: TransactionData,
) -> Result<Json<TxHash>, StatusCode> {
    let tx = Transaction::wrap(payload);
    let tx_hash = tx.hash();
    state
        .bus
        .sender::<RestApiMessage>()
        .await
        .send(RestApiMessage::NewTx(tx))
        .map(|_| tx_hash)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn send_contract_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<RegisterContractTransaction>,
) -> Result<impl IntoResponse, StatusCode> {
    handle_send(state, TransactionData::RegisterContract(payload)).await
}

pub async fn send_blob_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<BlobTransaction>,
) -> Result<impl IntoResponse, StatusCode> {
    handle_send(state, TransactionData::Blob(payload)).await
}

pub async fn send_proof_transaction(
    State(state): State<RouterState>,
    Json(payload): Json<ProofTransaction>,
) -> Result<impl IntoResponse, StatusCode> {
    handle_send(state, TransactionData::Proof(payload)).await
}

pub async fn get_contract(
    Path(name): Path<ContractName>,
    State(state): State<RouterState>,
) -> Result<impl IntoResponse, AppError> {
    let name_clone = name.clone();
    if let Some(res) = state
        .bus
        .request(NodeStateQuery::GetContract { name })
        .await?
    {
        match res {
            NodeStateQueryResponse::Contract { contract } => Ok(Json(contract)),
        }
    } else {
        Err(AppError(
            StatusCode::NOT_FOUND,
            anyhow!("Contract {} not found", name_clone),
        ))
    }
}

pub async fn run_scenario(
    State(state): State<RouterState>,
    Json(scenario): Json<RunScenario>,
) -> Result<impl IntoResponse, StatusCode> {
    state
        .bus
        .sender::<RunScenario>()
        .await
        .send(scenario)
        .map(|_| StatusCode::OK)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod test {
    use crate::model::{Blob, BlobData, BlobTransaction, ContractName, Identity};

    /*
        curl -X POST --location 'http://localhost:4321/v1/tx/send/blob' \
    --header 'Content-Type: application/json' \
    --data '{
        "identity": "ident",
        "blobs": [
            {
                "contract_name": "contrat de test",
                "data": []
            }
        ]
    }'
    */

    #[test]
    fn test_blob_tx_decode() {
        let payload_json = r#"
         {
            "identity": "ident",
            "blobs": [
                {
                    "contract_name": "contrat de test",
                    "data": []
                }
            ]
        }
         "#;

        let decoded: BlobTransaction = serde_json::from_str(payload_json).unwrap();

        assert_eq!(decoded.identity, Identity("ident".to_string()));
    }

    #[test]
    fn test_blob_tx_encode() {
        let payload = BlobTransaction {
            identity: Identity("tata".to_string()),
            blobs: vec![Blob {
                contract_name: ContractName("contract_name".to_string()),
                data: BlobData(vec![]),
            }],
        };

        let encoded = serde_json::to_string(&payload).unwrap();

        assert_eq!(
            encoded,
            "{\"identity\":\"tata\",\"blobs\":[{\"contract_name\":\"contract_name\",\"data\":[]}]}"
                .to_string()
        );
    }
}
