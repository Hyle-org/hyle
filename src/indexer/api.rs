use crate::model::{Blob, BlobData, ContractName, TxHash};

use super::{
    model::{BlobDb, BlockDb, ContractDb, ContractStateDb, TransactionDb, TransactionWithBlobs},
    IndexerState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use sqlx::Row;

// Blocks
pub async fn get_blocks(
    State(state): State<IndexerState>,
) -> Result<Json<Vec<BlockDb>>, StatusCode> {
    let blocks = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks")
        .fetch_all(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blocks.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blocks)),
    }
}

pub async fn get_last_block(
    State(state): State<IndexerState>,
) -> Result<Json<BlockDb>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks ORDER BY height DESC LIMIT 1")
        .fetch_optional(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_block(
    Path(height): Path<i64>,
    State(state): State<IndexerState>,
) -> Result<Json<BlockDb>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks WHERE height = $1")
        .bind(height)
        .fetch_optional(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_block_by_hash(
    Path(hash): Path<String>,
    State(state): State<IndexerState>,
) -> Result<Json<BlockDb>, StatusCode> {
    let block = sqlx::query_as::<_, BlockDb>("SELECT * FROM blocks WHERE hash = $1")
        .bind(hash)
        .fetch_optional(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match block {
        Some(block) => Ok(Json(block)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// Transactions
pub async fn get_transactions(
    State(state): State<IndexerState>,
) -> Result<Json<Vec<TransactionDb>>, StatusCode> {
    let transactions = sqlx::query_as::<_, TransactionDb>("SELECT * FROM transactions")
        .fetch_all(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transactions.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(transactions)),
    }
}

pub async fn get_transactions_with_contract_name(
    Path(contract_name): Path<String>,
    State(state): State<IndexerState>,
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
    .fetch_all(&state)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transactions.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(transactions)),
    }
}

pub async fn get_transactions_by_height(
    Path(height): Path<i64>,
    State(state): State<IndexerState>,
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
    .fetch_optional(&state)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transaction {
        Some(tx) => Ok(Json(tx)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_transaction_with_hash(
    Path(tx_hash): Path<String>,
    State(state): State<IndexerState>,
) -> Result<Json<TransactionDb>, StatusCode> {
    let transaction = sqlx::query_as::<_, TransactionDb>(
        r#"
        SELECT *
        FROM transactions
        WHERE tx_hash = $1
        "#,
    )
    .bind(tx_hash)
    .fetch_optional(&state)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match transaction {
        Some(tx) => Ok(Json(tx)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// Blobs
pub async fn get_blob_transactions_by_contract_name(
    Path(contract_name): Path<String>,
    State(state): State<IndexerState>,
) -> Result<Json<Vec<TransactionWithBlobs>>, StatusCode> {
    let rows = sqlx::query(
        r#"
        SELECT
            t.*,
            b.identity,
            b.contract_name,
            b.data
        FROM blobs b
        JOIN transactions t ON b.tx_hash = t.tx_hash
        WHERE b.tx_hash IN (
            SELECT tx_hash
            FROM blobs
            WHERE contract_name = $1
        )"#,
    )
    .bind(contract_name)
    .fetch_all(&state)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut transactions: Vec<TransactionWithBlobs> = Vec::new();
    let mut current_tx_hash: Option<TxHash> = None;
    let mut current_transaction: Option<TransactionWithBlobs> = None;

    for row in rows {
        let tx_hash: TxHash = row
            .try_get("tx_hash")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let contract_name: String = row
            .try_get("contract_name")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let data: Vec<u8> = row
            .try_get("data")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let blob = Blob {
            contract_name: ContractName(contract_name),
            data: BlobData(data),
        };

        if current_tx_hash.is_none() || current_tx_hash.as_ref() != Some(&tx_hash) {
            if let Some(transaction) = current_transaction.take() {
                transactions.push(transaction);
            }

            current_tx_hash = Some(tx_hash.clone());
            current_transaction = Some(TransactionWithBlobs {
                tx_hash: tx_hash.clone(),
                block_hash: row
                    .try_get("block_hash")
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                tx_index: row
                    .try_get("tx_index")
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                version: row
                    .try_get("version")
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                transaction_type: row
                    .try_get("transaction_type")
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                transaction_status: row
                    .try_get("transaction_status")
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                identity: row
                    .try_get("identity")
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                blobs: Vec::new(),
            });
        }

        if let Some(transaction) = &mut current_transaction {
            transaction.blobs.push(blob);
        }
    }

    if let Some(transaction) = current_transaction {
        transactions.push(transaction);
    }
    match transactions.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(transactions)),
    }
}

pub async fn get_blobs_by_contract_name(
    Path(contract_name): Path<String>,
    State(state): State<IndexerState>,
) -> Result<Json<Vec<BlobDb>>, StatusCode> {
    // TODO: Order transactions ?
    let blobs = sqlx::query_as::<_, BlobDb>(
        r#"
        SELECT b.*, t.transaction_status
        FROM blobs b
        JOIN transactions t ON b.tx_hash = t.tx_hash
        WHERE b.contract_name = $1
        "#,
    )
    .bind(contract_name)
    .fetch_all(&state)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blobs.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blobs)),
    }
}

pub async fn get_settled_blobs_by_contract_name(
    Path(contract_name): Path<String>,
    State(state): State<IndexerState>,
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
    .fetch_all(&state)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blobs.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blobs)),
    }
}

pub async fn get_unsettled_blobs_by_contract_name(
    Path(contract_name): Path<String>,
    State(state): State<IndexerState>,
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
    .fetch_all(&state)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blobs.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blobs)),
    }
}
pub async fn get_blobs_by_tx_hash(
    Path(tx_hash): Path<String>,
    State(state): State<IndexerState>,
) -> Result<Json<Vec<BlobDb>>, StatusCode> {
    // TODO: Order transaction ?
    let blobs = sqlx::query_as::<_, BlobDb>("SELECT * FROM blobs WHERE tx_hash = $1")
        .bind(tx_hash)
        .fetch_all(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match blobs.len() {
        0 => Err(StatusCode::NOT_FOUND),
        _ => Ok(Json(blobs)),
    }
}

pub async fn get_blob(
    Path((tx_hash, blob_index)): Path<(String, i32)>,
    State(state): State<IndexerState>,
) -> Result<Json<BlobDb>, StatusCode> {
    let blob =
        sqlx::query_as::<_, BlobDb>("SELECT * FROM blobs WHERE tx_hash = $1 AND blob_index = $2")
            .bind(tx_hash)
            .bind(blob_index)
            .fetch_optional(&state)
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
    State(state): State<IndexerState>,
) -> Result<Json<ContractDb>, StatusCode> {
    let contract =
        sqlx::query_as::<_, ContractDb>("SELECT * FROM contracts WHERE contract_name = $1")
            .bind(contract_name)
            .fetch_optional(&state)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match contract {
        Some(contract) => Ok(Json(contract)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn get_contract_state_by_height(
    Path((contract_name, height)): Path<(String, i64)>,
    State(state): State<IndexerState>,
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
    .fetch_optional(&state)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match contract {
        Some(contract) => Ok(Json(contract)),
        None => Err(StatusCode::NOT_FOUND),
    }
}
