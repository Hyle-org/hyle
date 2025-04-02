use super::IndexerApiState;
use api::{
    APIBlob, APIBlock, APIContract, APIContractState, APITransaction, APITransactionEvents,
    BlobWithStatus, TransactionStatusDb, TransactionTypeDb, TransactionWithBlobs,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use sqlx::Row;
use utoipa::OpenApi;

use crate::log_error;
use crate::model::*;

#[derive(Debug, serde::Deserialize)]
pub struct BlockPagination {
    pub start_block: Option<i64>,
    pub nb_results: Option<i64>,
}

#[derive(OpenApi)]
#[openapi(paths(get_blocks))]
pub(super) struct IndexerAPI;

#[utoipa::path(
    get,
    tag = "Indexer",
    path = "/blocks",
    responses(
        (status = OK, body = [APIBlock])
    )
)]
pub async fn get_blocks(
    Query(pagination): Query<BlockPagination>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APIBlock>>, StatusCode> {
    let blocks = match pagination.start_block {
        Some(start_block) => sqlx::query_as::<_, BlockDb>(
            "SELECT * FROM blocks WHERE height <= $1 and height > $2 ORDER BY height DESC LIMIT $3",
        )
        .bind(start_block)
        .bind(start_block - pagination.nb_results.unwrap_or(10)) // Fine if this goes negative
        .bind(pagination.nb_results.unwrap_or(10)),
        None => sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks ORDER BY height DESC LIMIT $1")
            .bind(pagination.nb_results.unwrap_or(10)),
    }
    .fetch_all(&state.db)
    .await
    .map(|db| db.into_iter().map(Into::<APIBlock>::into).collect())
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(blocks))
}

#[utoipa::path(
    get,
    tag = "Indexer",
    path = "/block/last",
    responses(
        (status = OK, body = APIBlock)
    )
)]
pub async fn get_last_block(
    State(state): State<IndexerApiState>,
) -> Result<Json<APIBlock>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks ORDER BY height DESC LIMIT 1")
        .fetch_optional(&state.db)
        .await
        .map(|db| db.map(Into::<APIBlock>::into))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    path = "/block/height/{height}",
    params(
        ("height" = String, Path, description = "Block height")
    ),
    responses(
        (status = OK, body = APIBlock)
    )
)]
pub async fn get_block(
    Path(height): Path<i64>,
    State(state): State<IndexerApiState>,
) -> Result<Json<APIBlock>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks WHERE height = $1")
        .bind(height)
        .fetch_optional(&state.db)
        .await
        .map(|db| db.map(Into::<APIBlock>::into))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    path = "/block/hash/{hash}",
    params(
        ("hash" = String, Path, description = "Block hash"),
    ),
    responses(
        (status = OK, body = APIBlock)
    )
)]
pub async fn get_block_by_hash(
    Path(hash): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<APIBlock>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks WHERE hash = $1")
        .bind(hash)
        .fetch_optional(&state.db)
        .await
        .map(|db| db.map(Into::<APIBlock>::into))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    path = "/transactions",
    responses(
        (status = OK, body = [APITransaction])
    )
)]
pub async fn get_transactions(
    Query(pagination): Query<BlockPagination>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APITransaction>>, StatusCode> {
    let transactions = match pagination.start_block {
        Some(start_block) => sqlx::query_as::<_, TransactionDb>(
            r#"
            SELECT t.*
            FROM transactions t
            LEFT JOIN blocks b ON t.block_hash = b.hash
            WHERE b.height <= $1 and b.height > $2 AND t.transaction_type = 'blob_transaction'
            ORDER BY b.height DESC, t.index ASC
            LIMIT $3
            "#,
        )
        .bind(start_block)
        .bind(start_block - pagination.nb_results.unwrap_or(10)) // Fine if this goes negative
        .bind(pagination.nb_results.unwrap_or(10)),
        None => sqlx::query_as::<_, TransactionDb>(
            r#"
            SELECT t.*
            FROM transactions t
            LEFT JOIN blocks b ON t.block_hash = b.hash
            WHERE t.transaction_type = 'blob_transaction'
            ORDER BY b.height DESC, t.index ASC
            LIMIT $1
            "#,
        )
        .bind(pagination.nb_results.unwrap_or(10)),
    }
    .fetch_all(&state.db)
    .await
    .map(|db| db.into_iter().map(Into::<APITransaction>::into).collect())
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(transactions))
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("contract_name" = String, Path, description = "Contract name"),
    ),
    path = "/transactions/contract/{contract_name}",
    responses(
        (status = OK, body = [APITransaction])
    )
)]
pub async fn get_transactions_by_contract(
    Path(contract_name): Path<String>,
    Query(pagination): Query<BlockPagination>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APITransaction>>, StatusCode> {
    let transactions = match pagination.start_block {
        Some(start_block) => sqlx::query_as::<_, TransactionDb>(
            r#"
            SELECT t.*
            FROM transactions t
            JOIN blobs b ON t.tx_hash = b.tx_hash
            LEFT JOIN blocks bl ON t.block_hash = bl.hash
            WHERE b.contract_name = $1 AND bl.height <= $2 AND bl.height > $3 AND t.transaction_type = 'blob_transaction'
            ORDER BY bl.height DESC, t.index ASC
            LIMIT $4
            "#,
        )
        .bind(contract_name)
        .bind(start_block)
        .bind(start_block - pagination.nb_results.unwrap_or(10)) // Fine if this goes negative
        .bind(pagination.nb_results.unwrap_or(10)),
        None => sqlx::query_as::<_, TransactionDb>(
            r#"
            SELECT t.*
            FROM transactions t
            JOIN blobs b ON t.tx_hash = b.tx_hash AND t.parent_dp_hash = b.parent_dp_hash
            WHERE b.contract_name = $1 AND t.transaction_type = 'blob_transaction'
            ORDER BY t.block_hash DESC, t.index ASC
            LIMIT $2
            "#,
        )
        .bind(contract_name)
        .bind(pagination.nb_results.unwrap_or(10)),
    }
    .fetch_all(&state.db)
    .await
    .map(|db| db.into_iter().map(Into::<APITransaction>::into).collect())
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // This could return 404 if the contract doesn't exist,
    // but not done for now as it would take an extra query
    Ok(Json(transactions))
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("height" = String, Path, description = "Block height")
    ),
    path = "/transactions/block/{height}",
    responses(
        (status = OK, body = [APITransaction])
    )
)]
pub async fn get_transactions_by_height(
    Path(height): Path<i64>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APITransaction>>, StatusCode> {
    let transactions = sqlx::query_as::<_, TransactionDb>(
        r#"
        SELECT t.*
        FROM transactions t
        JOIN blocks b ON t.block_hash = b.hash
        WHERE b.height = $1 AND t.transaction_type = 'blob_transaction'
        ORDER BY t.index ASC
        "#,
    )
    .bind(height)
    .fetch_all(&state.db)
    .await
    .map(|db| db.into_iter().map(Into::<APITransaction>::into).collect())
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(transactions))
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("tx_hash" = String, Path, description = "Tx hash"),
    ),
    path = "/transaction/hash/{tx_hash}",
    responses(
        (status = OK, body = APITransaction)
    )
)]
pub async fn get_transaction_with_hash(
    Path(tx_hash): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<APITransaction>, StatusCode> {
    let transaction = log_error!(sqlx::query_as::<_, TransactionDb>(
        r#"
        SELECT tx_hash, version, transaction_type, transaction_status, parent_dp_hash, block_hash, index
        FROM transactions
        WHERE tx_hash = $1 AND transaction_type = 'blob_transaction'
        ORDER BY index ASC
        "#,
    )
    .bind(tx_hash)
    .fetch_optional(&state.db)
    .await
    .map(|db| db.map(Into::<APITransaction>::into)),
    "Select transaction")
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transaction {
        Some(tx) => Ok(Json(tx)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("tx_hash" = String, Path, description = "Tx hash"),
    ),
    path = "/transaction/hash/{tx_hash}/events",
    responses(
        (status = OK, body = [APITransactionEvents])
    )
)]
pub async fn get_transaction_events(
    Path(tx_hash): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APITransactionEvents>>, StatusCode> {
    let rows = log_error!(
        sqlx::query(
            r#"
        SELECT t.block_hash, b.height, t.tx_hash, t.events
        FROM transaction_state_events t
        LEFT JOIN blocks b ON t.block_hash = b.hash
        WHERE tx_hash = $1
        ORDER BY (b.height, index) DESC;
        "#,
        )
        .bind(tx_hash)
        .fetch_all(&state.db)
        .await,
        "Failed to fetch transactions with blobs"
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let transactions: Result<Vec<APITransactionEvents>, anyhow::Error> = rows
        .into_iter()
        .map(|row| {
            let block_hash = row.try_get("block_hash")?;
            let block_height: i64 = row.try_get("height")?;
            let block_height = BlockHeight(block_height.try_into()?);
            let events: serde_json::Value = row.try_get("events")?;
            let events: Vec<serde_json::Value> = serde_json::from_value(events)?;
            Ok(APITransactionEvents {
                block_hash,
                block_height,
                events,
            })
        })
        .collect();

    match transactions {
        Ok(transactions) => Ok(Json(transactions)),
        Err(e) => {
            tracing::warn!("Failed to parse transactions with blobs: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("contract_name" = String, Path, description = "Contract name"),
    ),
    path = "/blob_transactions/contract/{contract_name}",
    responses(
        (status = OK, body = [TransactionWithBlobs])
    )
)]
pub async fn get_blob_transactions_by_contract(
    Path(contract_name): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<TransactionWithBlobs>>, StatusCode> {
    let rows = log_error!(sqlx::query(
        r#"
        with blobs as (
            SELECT blobs.*, array_remove(ARRAY_AGG(blob_proof_outputs.hyle_output), NULL) AS proof_outputs
            FROM blobs
            LEFT JOIN blob_proof_outputs ON blobs.parent_dp_hash = blob_proof_outputs.blob_parent_dp_hash AND blobs.tx_hash = blob_proof_outputs.blob_tx_hash AND blobs.blob_index = blob_proof_outputs.blob_index
            WHERE blobs.contract_name = $1
            GROUP BY blobs.parent_dp_hash, blobs.tx_hash, blobs.blob_index, blobs.identity
        )
        SELECT
            t.tx_hash,
            t.parent_dp_hash,
            t.block_hash,
            t.index,
            t.version,
            t.transaction_type,
            t.transaction_status,
            b.identity,
            array_agg(ROW(b.contract_name, b.data, b.proof_outputs)) AS blobs
        FROM blobs b
        JOIN transactions t on t.tx_hash = b.tx_hash AND t.parent_dp_hash = b.parent_dp_hash
        GROUP BY
            t.tx_hash,
            t.parent_dp_hash,
            t.block_hash,
            t.index,
            t.version,
            t.transaction_type,
            t.transaction_status,
            b.identity
        "#,
    )
    .bind(contract_name.clone())
    .fetch_all(&state.db)
    .await, "Failed to fetch transactions with blobs")
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let transactions: Result<Vec<TransactionWithBlobs>, anyhow::Error> = rows
        .into_iter()
        .map(|row| {
            let tx_hash: TxHashDb = row.try_get("tx_hash")?;
            let dp_hash: DataProposalHashDb = row.try_get("parent_dp_hash")?;
            let block_hash: ConsensusProposalHash = row.try_get("block_hash")?;
            let index: i32 = row.try_get("index")?;
            let version: i32 = row.try_get("version")?;
            let transaction_type: TransactionTypeDb = row.try_get("transaction_type")?;
            let transaction_status: TransactionStatusDb = row.try_get("transaction_status")?;
            let identity: String = row.try_get("identity")?;
            let blobs: Vec<(String, Vec<u8>, Vec<serde_json::Value>)> = row.try_get("blobs")?;

            let index: u32 = index.try_into()?;
            let version: u32 = version.try_into()?;

            let blobs = blobs
                .into_iter()
                .map(|(contract_name, data, proof_outputs)| BlobWithStatus {
                    contract_name,
                    data,
                    proof_outputs,
                })
                .collect();

            Ok(TransactionWithBlobs {
                tx_hash: tx_hash.0,
                parent_dp_hash: dp_hash.0,
                block_hash,
                index,
                version,
                transaction_type,
                transaction_status,
                identity,
                blobs,
            })
        })
        .collect();
    match transactions {
        Ok(transactions) => Ok(Json(transactions)),
        Err(e) => {
            tracing::warn!("Failed to parse transactions with blobs: {:?}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("tx_hash" = String, Path, description = "Tx hash"),
    ),
    path = "/blobs/hash/{tx_hash}",
    responses(
        (status = OK, body = [APIBlob])
    )
)]
pub async fn get_blobs_by_tx_hash(
    Path(tx_hash): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APIBlob>>, StatusCode> {
    // TODO: Order transaction ?
    let blobs = sqlx::query_as::<_, BlobDb>(
        r#"
        SELECT blobs.*, array_remove(ARRAY_AGG(blob_proof_outputs.hyle_output), NULL) AS proof_outputs
        FROM blobs
        LEFT JOIN blob_proof_outputs ON blobs.parent_dp_hash = blob_proof_outputs.blob_parent_dp_hash 
            AND blobs.tx_hash = blob_proof_outputs.blob_tx_hash 
            AND blobs.blob_index = blob_proof_outputs.blob_index
        WHERE blobs.tx_hash = $1
        GROUP BY blobs.parent_dp_hash, blobs.tx_hash, blobs.blob_index, blobs.identity
        "#,
    )
    .bind(tx_hash)
    .fetch_all(&state.db)
    .await
    .map(|db| db.into_iter().map(Into::<APIBlob>::into).collect())
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // This could return 404 if the transaction doesn't exist,
    // but not done for now as it would take an extra query
    Ok(Json(blobs))
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("tx_hash" = String, Path, description = "Tx hash"),
        ("blob_index" = String, Path, description = "Blob index"),
    ),
    path = "/blob/hash/{tx_hash}/index/{blob_index}",
    responses(
        (status = OK, body = APIBlob)
    )
)]
pub async fn get_blob(
    Path((tx_hash, blob_index)): Path<(String, i32)>,
    State(state): State<IndexerApiState>,
) -> Result<Json<APIBlob>, StatusCode> {
    let blob = sqlx::query_as::<_, BlobDb>(
        r#"
        SELECT blobs.*, array_remove(ARRAY_AGG(blob_proof_outputs.hyle_output), NULL) AS proof_outputs
        FROM blobs
        LEFT JOIN blob_proof_outputs ON blobs.parent_dp_hash = blob_proof_outputs.blob_parent_dp_hash 
            AND blobs.tx_hash = blob_proof_outputs.blob_tx_hash 
            AND blobs.blob_index = blob_proof_outputs.blob_index
        WHERE blobs.tx_hash = $1 AND blobs.blob_index = $2
        GROUP BY blobs.parent_dp_hash, blobs.tx_hash, blobs.blob_index, blobs.identity
        "#,
    )
    .bind(tx_hash)
    .bind(blob_index)
    .fetch_optional(&state.db)
    .await
    .map(|db| db.map(Into::<APIBlob>::into))
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blob {
        Some(blob) => Ok(Json(blob)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    path = "/contracts",
    responses(
        (status = OK, body = [APIContract])
    )
)]
pub async fn list_contracts(
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APIContract>>, StatusCode> {
    let contract = sqlx::query_as::<_, ContractDb>("SELECT * FROM contracts")
        .fetch_all(&state.db)
        .await
        .map(|db| db.into_iter().map(Into::<APIContract>::into).collect())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(contract))
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("contract_name" = String, Path, description = "Contract name"),
    ),
    path = "/contract/{contract_name}",
    responses(
        (status = OK, body = APIContract)
    )
)]
pub async fn get_contract(
    Path(contract_name): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<APIContract>, StatusCode> {
    let contract =
        sqlx::query_as::<_, ContractDb>("SELECT * FROM contracts WHERE contract_name = $1")
            .bind(contract_name)
            .fetch_optional(&state.db)
            .await
            .map(|db| db.map(Into::<APIContract>::into))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match contract {
        Some(contract) => Ok(Json(contract)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("contract_name" = String, Path, description = "Contract name"),
        ("height" = String, Path, description = "Block height")
    ),
    path = "/state/contract/{contract_name}/block/{height}",
    responses(
        (status = OK, body = APIContractState)
    )
)]
pub async fn get_contract_state_by_height(
    Path((contract_name, height)): Path<(String, i64)>,
    State(state): State<IndexerApiState>,
) -> Result<Json<APIContractState>, StatusCode> {
    let contract = sqlx::query_as::<_, ContractStateDb>(
        r#"
        SELECT cs.*
        FROM contract_state cs
        JOIN blocks b ON cs.block_hash = b.hash
        WHERE contract_name = $1 AND height = $2"#,
    )
    .bind(contract_name)
    .bind(height)
    .fetch_optional(&state.db)
    .await
    .map(|db| db.map(Into::<APIContractState>::into))
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match contract {
        Some(contract) => Ok(Json(contract)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

#[utoipa::path(
    get,
    tag = "Indexer",
    path = "/proofs",
    responses(
        (status = OK, body = [APITransaction])
    )
)]
pub async fn get_proofs(
    Query(pagination): Query<BlockPagination>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APITransaction>>, StatusCode> {
    let transactions = match pagination.start_block {
        Some(start_block) => sqlx::query_as::<_, TransactionDb>(
            r#"
            SELECT t.*
            FROM transactions t
            LEFT JOIN blocks b ON t.block_hash = b.hash
            WHERE b.height <= $1 and b.height > $2 AND t.transaction_type = 'proof_transaction'
            ORDER BY b.height DESC, t.index ASC
            LIMIT $3
            "#,
        )
        .bind(start_block)
        .bind(start_block - pagination.nb_results.unwrap_or(10)) // Fine if this goes negative
        .bind(pagination.nb_results.unwrap_or(10)),
        None => sqlx::query_as::<_, TransactionDb>(
            r#"
            SELECT t.*
            FROM transactions t
            LEFT JOIN blocks b ON t.block_hash = b.hash
            WHERE t.transaction_type = 'proof_transaction'
            ORDER BY b.height DESC, t.index ASC
            LIMIT $1
            "#,
        )
        .bind(pagination.nb_results.unwrap_or(10)),
    }
    .fetch_all(&state.db)
    .await
    .map(|db| db.into_iter().map(Into::<APITransaction>::into).collect())
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(transactions))
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("height" = String, Path, description = "Block height")
    ),
    path = "/proofs/block/{height}",
    responses(
        (status = OK, body = [APITransaction])
    )
)]
pub async fn get_proofs_by_height(
    Path(height): Path<i64>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<APITransaction>>, StatusCode> {
    let transactions = sqlx::query_as::<_, TransactionDb>(
        r#"
        SELECT t.*
        FROM transactions t
        JOIN blocks b ON t.block_hash = b.hash
        WHERE b.height = $1 AND t.transaction_type = 'proof_transaction'
        ORDER BY t.index ASC
        "#,
    )
    .bind(height)
    .fetch_all(&state.db)
    .await
    .map(|db| db.into_iter().map(Into::<APITransaction>::into).collect())
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(transactions))
}

#[utoipa::path(
    get,
    tag = "Indexer",
    params(
        ("tx_hash" = String, Path, description = "Tx hash")
    ),
    path = "/proof/hash/{tx_hash}",
    responses(
        (status = OK, body = APITransaction)
    )
)]
pub async fn get_proof_with_hash(
    Path(tx_hash): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<APITransaction>, StatusCode> {
    let transaction = log_error!(sqlx::query_as::<_, TransactionDb>(
        r#"
        SELECT tx_hash, version, transaction_type, transaction_status, parent_dp_hash, block_hash, index
        FROM transactions
        WHERE tx_hash = $1 AND transaction_type = 'proof_transaction'
        ORDER BY index ASC
        "#,
    )
    .bind(tx_hash)
    .fetch_optional(&state.db)
    .await
    .map(|db| db.map(Into::<APITransaction>::into)),
    "Select transaction")
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transaction {
        Some(tx) => Ok(Json(tx)),
        None => Err(StatusCode::NOT_FOUND),
    }
}
