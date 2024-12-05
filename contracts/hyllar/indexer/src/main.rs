use anyhow::Result;
use hyle::{
    model::BlobTransaction,
    rest::{NodeInfo, RestApi, RestApiRunContext, Router},
    tools::contract_state_indexer::{ContractStateIndexer, ContractStateIndexerCtx},
    utils::{
        logger::{setup_tracing, TracingMode},
        modules::ModulesHandler,
    },
};
use hyllar::{HyllarToken, HyllarTokenContract};
use sdk::{erc20::ERC20Action, Blob, BlobIndex, StructuredBlobData};
use std::{
    env,
    sync::{Arc, Mutex},
};
use tracing::{error, info};

fn handle_blob_data(
    tx: &BlobTransaction,
    index: BlobIndex,
    state: HyllarToken,
) -> Result<HyllarToken> {
    let Blob {
        contract_name,
        data,
    } = tx.blobs.get(index.0 as usize).unwrap();

    let data: StructuredBlobData<ERC20Action> = data.clone().try_into()?;

    let caller: sdk::Identity = data
        .caller
        .map(|i| {
            tx.blobs
                .get(i.0 as usize)
                .unwrap()
                .contract_name
                .0
                .clone()
                .into()
        })
        .unwrap_or(tx.identity.clone());

    let mut contract = HyllarTokenContract::init(state, caller);
    let res = sdk::erc20::execute_action(&mut contract, data.parameters);
    info!("🚀 Executed {contract_name}: {res:?}");
    Ok(contract.state())
}

#[tokio::main]
async fn main() -> Result<()> {
    let da_url = env::var("DA_URL").unwrap_or_else(|_| "localhost:4141".to_string());
    let log_format = env::var("LOG_FORMAT").unwrap_or_else(|_| "full".to_string());
    let rest_addr = env::var("HOST_URL").unwrap_or_else(|_| "127.0.0.1:8010".to_string());

    let id = "hyllar_indexer".to_string();

    setup_tracing(
        match log_format.as_str() {
            "json" => TracingMode::Json,
            "node" => TracingMode::NodeName,
            _ => TracingMode::Full,
        },
        id.clone(),
    )?;

    let mut handler = ModulesHandler::default();

    let router = Arc::new(Mutex::new(Some(Router::new())));

    let ctx2 = ContractStateIndexerCtx {
        da_address: da_url.clone(),
        program_id: include_str!("../../hyllar.txt").trim().to_string(),
        handler: Box::new(handle_blob_data),
        router: Arc::clone(&router),
    };

    handler
        .build_module::<ContractStateIndexer<HyllarToken>>(ctx2)
        .await?;

    let rest_ctx = RestApiRunContext::new(
        id.clone(),
        rest_addr,
        NodeInfo {
            id,
            pubkey: None,
            da_address: da_url,
        },
        router.lock().unwrap().take().unwrap(),
    );
    handler.build_module::<RestApi>(rest_ctx).await?;

    let (running_modules, abort) = handler.start_modules()?;

    #[cfg(unix)]
    {
        use tokio::signal::unix;
        let mut terminate = unix::signal(unix::SignalKind::interrupt())?;
        tokio::select! {
            Err(e) = running_modules => {
                error!("Error running modules: {:?}", e);
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down");
                abort();
            }
            _ = terminate.recv() =>  {
                info!("SIGTERM received, shutting down");
                abort();
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            Err(e) = running_modules => {
                error!("Error running modules: {:?}", e);
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down");
                abort();
            }
        }
    }

    Ok(())
}
