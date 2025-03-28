use std::{env, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{Json, State},
    http::Method,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use client_sdk::{
    contract_states,
    rest_client::{IndexerApiHttpClient, NodeApiHttpClient},
    transaction_builder::{ProvableBlobTx, TxExecutor, TxExecutorBuilder},
};
use passport::Passport;
use ponzhyle::Ponzhyle;
use reqwest::{Client, Url};
use sdk::{guest, BlobTransaction, ContractInput, ContractName, HyleOutput, TxHash};
use serde::Deserialize;
use task_manager::Prover;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use twitter::Twitter;
use utils::AppError;

mod task_manager;
mod utils;

#[derive(Clone)]
struct RouterCtx {
    pub app: Arc<Mutex<HyleOofCtx>>,
}

async fn build_app_context(
    indexer: Arc<IndexerApiHttpClient>,
    node: Arc<NodeApiHttpClient>,
) -> HyleOofCtx {
    let ponzhyle = indexer
        .fetch_current_state(&"ponzhyle".into())
        .await
        .unwrap();
    let passport: Passport = Passport::default();
    let twitter: Twitter = Twitter::default();

    let executor = TxExecutorBuilder::new(States {
        ponzhyle,
        passport,
        twitter,
    })
    .build();

    HyleOofCtx {
        executor,
        client: node.clone(),
        prover: Arc::new(Prover::new(node)),
        ponzhyle_cn: "ponzhyle".into(),
        passport_cn: "passport".into(),
        twitter_cn: "twitter".into(),
    }
}

fn setup_tracing() {
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()
        .unwrap();
    let var = std::env::var("RUST_LOG").unwrap_or("".to_string());
    if !var.contains("risc0_zkvm") {
        filter = filter.add_directive("risc0_zkvm=info".parse().unwrap());
        filter = filter.add_directive("risc0_circuit_rv32im=info".parse().unwrap());
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter))
        .init();
}

#[tokio::main]
async fn main() {
    setup_tracing();

    let node_url = env::var("NODE_URL").unwrap_or_else(|_| "http://localhost:4321".to_string());
    let indexer_url =
        env::var("INDEXER_URL").unwrap_or_else(|_| "http://localhost:4321".to_string());
    let node_client = Arc::new(NodeApiHttpClient {
        url: Url::parse(node_url.as_str()).unwrap(),
        reqwest_client: Client::new(),
        api_key: None,
    });
    let indexer_client = Arc::new(IndexerApiHttpClient {
        url: Url::parse(indexer_url.as_str()).unwrap(),
        reqwest_client: Client::new(),
    });

    let state = RouterCtx {
        app: Arc::new(Mutex::new(
            build_app_context(indexer_client, node_client).await,
        )),
    };

    // Créer un middleware CORS
    let cors = CorsLayer::new()
        .allow_origin(Any) // Permet toutes les origines (peut être restreint)
        .allow_methods(vec![Method::GET, Method::POST]) // Permet les méthodes nécessaires
        .allow_headers(Any); // Permet tous les en-têtes
    let app = Router::new()
        .route("/_health", get(health))
        .route("/api/send_invite", post(send_invite))
        .route("/api/redeem_invite", post(redeem_invite))
        .route("/api/register_nationality", post(register_nationality))
        .with_state(state)
        .layer(cors); // Appliquer le middleware CORS

    let addr: String = env::var("HYLEOOF_HOST")
        .unwrap_or_else(|_| "127.0.0.1:3001".to_string())
        .parse()
        .unwrap();
    info!("Server running on {}", addr);
    _ = axum::serve(tokio::net::TcpListener::bind(&addr).await.unwrap(), app).await;
}

async fn health() -> impl IntoResponse {
    Json("OK")
}

// --------------------------------------------------------
//      Send Invite
// --------------------------------------------------------

#[derive(Deserialize)]
struct SendInviteRequest {
    #[serde(alias = "currentUser")]
    current_user: String,
    #[serde(alias = "inviteHandle")]
    invite_handle: String,
}

async fn send_invite(
    State(ctx): State<RouterCtx>,
    Json(payload): Json<SendInviteRequest>,
) -> Result<impl IntoResponse, AppError> {
    tracing::info!(
        "Sending invite from {} to {}",
        payload.current_user,
        payload.invite_handle
    );
    let tx_hash = do_send_invite(ctx, payload.current_user, payload.invite_handle).await?;
    Ok(Json(tx_hash))
}

async fn do_send_invite(
    ctx: RouterCtx,
    current_user: String,
    invite_handle: String,
) -> Result<TxHash, AppError> {
    let mut app = ctx.app.lock_owned().await;
    let mut transaction = ProvableBlobTx::new(current_user.into());

    app.verify_identity(&mut transaction)?;
    app.send_invite(&mut transaction, invite_handle)?;

    app.send(transaction).await
}

// // --------------------------------------------------------
// //      Redeem Invite
// // --------------------------------------------------------

#[derive(Deserialize)]
struct RedeemInviteRequest {
    username: String,
    referrer: String,
}

async fn redeem_invite(
    State(ctx): State<RouterCtx>,
    Json(payload): Json<RedeemInviteRequest>,
) -> Result<impl IntoResponse, AppError> {
    let tx_hash = do_redeem_invite(ctx, payload.username, payload.referrer).await?;
    Ok(Json(tx_hash))
}

async fn do_redeem_invite(
    ctx: RouterCtx,
    username: String,
    referrer: String,
) -> Result<TxHash, AppError> {
    let mut app = ctx.app.lock_owned().await;
    let mut transaction = ProvableBlobTx::new(username.into());

    app.verify_identity(&mut transaction)?;
    app.redeem_invite(&mut transaction, referrer)?;

    app.send(transaction).await
}

// // --------------------------------------------------------
// //    Register Nationality
// // --------------------------------------------------------

#[derive(Deserialize)]
struct RegisterNationalityRequest {
    username: String,
    nationality: String,
}

async fn register_nationality(
    State(ctx): State<RouterCtx>,
    Json(payload): Json<RegisterNationalityRequest>,
) -> Result<impl IntoResponse, AppError> {
    let tx_hash = do_register_nationality(ctx, payload.username, payload.nationality).await?;
    Ok(Json(tx_hash))
}

async fn do_register_nationality(
    ctx: RouterCtx,
    username: String,
    nationality: String,
) -> Result<TxHash, AppError> {
    let mut app = ctx.app.lock_owned().await;
    let mut transaction = ProvableBlobTx::new(username.into());

    app.verify_identity(&mut transaction)?;
    app.register_nationality(&mut transaction, nationality)?;

    app.send(transaction).await
}

contract_states!(
    pub struct States {
        pub ponzhyle: Ponzhyle,
        pub passport: Passport,
        pub twitter: Twitter,
    }
);

struct HyleOofCtx {
    executor: TxExecutor<States>,
    client: Arc<NodeApiHttpClient>,
    prover: Arc<Prover>,
    ponzhyle_cn: ContractName,
    passport_cn: ContractName,
    twitter_cn: ContractName,
}

impl HyleOofCtx {
    async fn send(&mut self, transaction: ProvableBlobTx) -> Result<TxHash, AppError> {
        let blob_tx = BlobTransaction::new(transaction.identity.clone(), transaction.blobs.clone());

        let proof_tx_builder = self.executor.process(transaction)?;

        let tx_hash = self.client.send_tx_blob(&blob_tx).await?;

        self.prover.add(proof_tx_builder).await;

        Ok(tx_hash)
    }

    fn verify_identity(&mut self, transaction: &mut ProvableBlobTx) -> Result<()> {
        twitter::client::verify_identity(transaction, self.twitter_cn.clone())
    }

    fn send_invite(&mut self, transaction: &mut ProvableBlobTx, recipient: String) -> Result<()> {
        ponzhyle::client::send_invite(transaction, self.ponzhyle_cn.clone(), recipient)
    }

    fn redeem_invite(&mut self, transaction: &mut ProvableBlobTx, referrer: String) -> Result<()> {
        ponzhyle::client::redeem_invite(transaction, self.ponzhyle_cn.clone(), referrer)
    }

    fn register_nationality(
        &mut self,
        transaction: &mut ProvableBlobTx,
        nationality: String,
    ) -> Result<()> {
        ponzhyle::client::register_nationality(transaction, self.ponzhyle_cn.clone(), nationality)
    }
}
