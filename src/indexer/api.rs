use crate::model::{Blob, BlobData, BlockHash, ContractName};

use super::{
    model::{
        BlobDb, BlobDbWithStatus, BlockDb, ContractDb, ContractStateDb, TransactionDb,
        TransactionStatus, TransactionType, TransactionWithBlobs, TxHashDb,
    },
    IndexerApiState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use sqlx::Row;

// Blocks
pub async fn get_blocks(
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<BlockDb>>, StatusCode> {
    let blocks = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks")
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blocks.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blocks)),
    }
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
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<TransactionDb>>, StatusCode> {
    let transactions = sqlx::query_as::<_, TransactionDb>("SELECT * FROM transactions")
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transactions.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(transactions)),
    }
}

pub async fn get_transactions_by_contract(
    Path(contract_name): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<TransactionDb>>, StatusCode> {
    let transactions = sqlx::query_as::<_, TransactionDb>(
        r#"
        SELECT t.*
        FROM transactions t
        JOIN blobs b ON t.tx_hash = b.tx_hash
        WHERE b.contract_name = $1
        "#,
    )
    .bind(contract_name)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transactions.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(transactions)),
    }
}

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
        SELECT
            t.tx_hash,
            t.block_hash,
            t.version,
            t.transaction_type,
            t.transaction_status,
            b.identity,
            ARRAY_AGG(ROW(b.contract_name, b.data)) AS blobs
        FROM transactions t
        JOIN blobs b ON t.tx_hash = b.tx_hash
        WHERE b.tx_hash IN (
            SELECT tx_hash
            FROM blobs
            WHERE contract_name = $1)
        GROUP BY
            t.tx_hash,
            t.block_hash,
            t.version,
            t.transaction_type,
            t.transaction_status,
            b.identity
        "#,
    )
    .bind(contract_name)
    .fetch_all(&state.db)
    .await
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
            let blobs: Vec<(String, Vec<u8>)> = row.try_get("blobs").unwrap();

            let blobs = blobs
                .into_iter()
                .map(|(contract_name, data)| Blob {
                    contract_name: ContractName(contract_name),
                    data: BlobData(data),
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

    match transactions.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(transactions)),
    }
}

pub async fn get_blobs_by_contract(
    Path(contract_name): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<BlobDbWithStatus>>, StatusCode> {
    // TODO: Order transactions ?
    let blobs = sqlx::query_as::<_, BlobDbWithStatus>(
        r#"
        SELECT b.*, t.transaction_status
        FROM blobs b
        JOIN transactions t ON b.tx_hash = t.tx_hash
        WHERE b.contract_name = $1
        "#,
    )
    .bind(contract_name)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blobs.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blobs)),
    }
}

pub async fn get_settled_blobs_by_contract(
    Path(contract_name): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<BlobDb>>, StatusCode> {
    // TODO: Order transactions ?
    let blobs = sqlx::query_as::<_, BlobDb>(
        r#"
        SELECT b.*
        FROM blobs b
        JOIN transactions t ON b.tx_hash = t.tx_hash
        WHERE b.contract_name = $1 AND t.transaction_status = 'success'
        "#,
    )
    .bind(contract_name)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blobs.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blobs)),
    }
}

pub async fn get_unsettled_blobs_by_contract(
    Path(contract_name): Path<String>,
    State(state): State<IndexerApiState>,
) -> Result<Json<Vec<BlobDb>>, StatusCode> {
    // TODO: Order transaction ?
    let blobs = sqlx::query_as::<_, BlobDb>(
        r#"
        SELECT b.*
        FROM blobs b
        JOIN transactions t ON b.tx_hash = t.tx_hash
        WHERE b.contract_name = $1 AND t.transaction_status = 'sequenced'
        "#,
    )
    .bind(contract_name)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blobs.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blobs)),
    }
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

    match blobs.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blobs)),
    }
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
