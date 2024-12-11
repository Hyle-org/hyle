use crate::{model::BlockHash, utils::logger::LogMe};

use super::{
    model::{
        BlobDb, BlobWithStatus, BlockDb, ContractDb, ContractStateDb, TransactionDb,
        TransactionStatus, TransactionType, TransactionWithBlobs, TxHashDb,
    },
    IndexerApiState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use sqlx::Row;

#[derive(Debug, serde::Deserialize)]
pub struct BlockPagination {
    pub start_block: Option<i64>,
    pub nb_results: Option<i64>,
}

// Blocks
pub async fn get_blocks(
    Query(pagination): Query<BlockPagination>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<BlockDb>>, StatusCode> {
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
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(blocks))
}

pub async fn get_last_block(
    State(state): State<IndexerApiState>,
) -> Result<Json<BlockDb>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks ORDER BY height DESC LIMIT 1")
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_block(
    Path(height): Path<i64>,
    State(state): State<IndexerApiState>,
) -> Result<Json<BlockDb>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks WHERE height = $1")
        .bind(height)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_block_by_hash(
    Path(hash): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<BlockDb>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks WHERE hash = $1")
        .bind(hash)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// Transactions
pub async fn get_transactions(
    Query(pagination): Query<BlockPagination>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<TransactionDb>>, StatusCode> {
    let transactions = match pagination.start_block {
        Some(start_block) => sqlx::query_as::<_, TransactionDb>(
            r#"
            SELECT t.*
            FROM transactions t
            JOIN blocks b ON t.block_hash = b.hash
            WHERE b.height <= $1 and b.height > $2
            ORDER BY b.height DESC
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
            JOIN blocks b ON t.block_hash = b.hash
            ORDER BY b.height DESC
            LIMIT $1
            "#,
        )
        .bind(pagination.nb_results.unwrap_or(10)),
    }
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(transactions))
}

pub async fn get_transactions_by_contract(
    Path(contract_name): Path<String>,
    Query(pagination): Query<BlockPagination>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<TransactionDb>>, StatusCode> {
    let transactions = match pagination.start_block {
        Some(start_block) => sqlx::query_as::<_, TransactionDb>(
            r#"
            SELECT t.*
            FROM transactions t
            JOIN blobs b ON t.tx_hash = b.tx_hash
            JOIN blocks bl ON t.block_hash = bl.hash
            WHERE b.contract_name = $1 AND bl.height <= $2 AND bl.height > $3
            ORDER BY bl.height DESC
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
            JOIN blobs b ON t.tx_hash = b.tx_hash
            WHERE b.contract_name = $1
            ORDER BY t.block_hash DESC
            LIMIT $2
            "#,
        )
        .bind(contract_name)
        .bind(pagination.nb_results.unwrap_or(10)),
    }
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(transactions))
}

// TODO: pagination ?
pub async fn get_transactions_by_height(
    Path(height): Path<i64>,
    State(state): State<IndexerApiState>,
) -> Result<Json<TransactionDb>, StatusCode> {
    let transaction = sqlx::query_as::<_, TransactionDb>(
        r#"
        SELECT t.*
        FROM transactions t
        JOIN blocks b ON t.block_hash = b.hash
        WHERE b.height = $1
        "#,
    )
    .bind(height)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transaction {
        Some(tx) => Ok(Json(tx)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_transaction_with_hash(
    Path(tx_hash): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<TransactionDb>, StatusCode> {
    let transaction = sqlx::query_as::<_, TransactionDb>(
        r#"
        SELECT *
        FROM transactions
        WHERE tx_hash = $1
        "#,
    )
    .bind(tx_hash)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transaction {
        Some(tx) => Ok(Json(tx)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// Blobs
pub async fn get_blob_transactions_by_contract(
    Path(contract_name): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<TransactionWithBlobs>>, StatusCode> {
    let rows = sqlx::query(
        r#"
        with blobs as (
            SELECT blobs.*, array_remove(ARRAY_AGG(proofs.hyle_output), NULL) AS proof_outputs
            FROM blobs
            LEFT JOIN proofs ON blobs.tx_hash = proofs.blob_tx_hash AND blobs.blob_index = proofs.blob_index
            WHERE blobs.contract_name = $1
            GROUP BY blobs.tx_hash, blobs.blob_index, blobs.identity
        )
        SELECT
            t.tx_hash,
            t.block_hash,
            t.version,
            t.transaction_type,
            t.transaction_status,
            b.identity,
            array_agg(ROW(b.contract_name, b.data, b.proof_outputs)) AS blobs
        FROM blobs b
        JOIN transactions t on t.tx_hash = b.tx_hash
        GROUP BY
            t.tx_hash,
            t.block_hash,
            t.version,
            t.transaction_type,
            t.transaction_status,
            b.identity
        "#,
    )
    .bind(contract_name.clone())
    .fetch_all(&state.db)
    .await
    .log_error("Failed to fetch transactions with blobs")
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let transactions: Vec<TransactionWithBlobs> = rows
        .into_iter()
        .map(|row| {
            let tx_hash: TxHashDb = row.try_get("tx_hash").unwrap();
            let block_hash: BlockHash = row.try_get("block_hash").unwrap();
            let version: i32 = row.try_get("version").unwrap();
            let transaction_type: TransactionType = row.try_get("transaction_type").unwrap();
            let transaction_status: TransactionStatus = row.try_get("transaction_status").unwrap();
            let identity: String = row.try_get("identity").unwrap();
            let blobs: Vec<(String, Vec<u8>, Vec<serde_json::Value>)> =
                row.try_get("blobs").unwrap();

            let blobs = blobs
                .into_iter()
                .map(|(contract_name, data, proof_outputs)| BlobWithStatus {
                    contract_name,
                    data,
                    proof_outputs,
                })
                .collect();

            TransactionWithBlobs {
                tx_hash,
                block_hash,
                version,
                transaction_type,
                transaction_status,
                identity,
                blobs,
            }
        })
        .collect();

    Ok(Json(transactions))
}

pub async fn get_blobs_by_tx_hash(
    Path(tx_hash): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<BlobDb>>, StatusCode> {
    // TODO: Order transaction ?
    let blobs = sqlx::query_as::<_, BlobDb>("SELECT * FROM blobs WHERE tx_hash = $1")
        .bind(tx_hash)
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(blobs))
}

pub async fn get_blob(
    Path((tx_hash, blob_index)): Path<(String, i32)>,
    State(state): State<IndexerApiState>,
) -> Result<Json<BlobDb>, StatusCode> {
    let blob =
        sqlx::query_as::<_, BlobDb>("SELECT * FROM blobs WHERE tx_hash = $1 AND blob_index = $2")
            .bind(tx_hash)
            .bind(blob_index)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blob {
        Some(blob) => Ok(Json(blob)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// Contracts
pub async fn list_contracts(
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<ContractDb>>, StatusCode> {
    let contract = sqlx::query_as::<_, ContractDb>("SELECT * FROM contracts")
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(contract))
}

pub async fn get_contract(
    Path(contract_name): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<ContractDb>, StatusCode> {
    let contract =
        sqlx::query_as::<_, ContractDb>("SELECT * FROM contracts WHERE contract_name = $1")
            .bind(contract_name)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match contract {
        Some(contract) => Ok(Json(contract)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_contract_state_by_height(
    Path((contract_name, height)): Path<(String, i64)>,
    State(state): State<IndexerApiState>,
) -> Result<Json<ContractStateDb>, StatusCode> {
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
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match contract {
        Some(contract) => Ok(Json(contract)),
        None => Err(StatusCode::NOT_FOUND),
    }
}
