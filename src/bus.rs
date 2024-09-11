use anymap::{any::Any, Map};
use std::sync::{atomic::AtomicUsize, Arc};
use tokio::sync::{broadcast, Mutex};
use tracing::info;

use crate::{mempool::MempoolCommand, model::Transaction};

pub mod command_response;
pub mod listener;

pub const CHANNEL_CAPACITY: usize = 1024;

type AnyMap = Map<dyn Any + Send + Sync>;

pub struct SharedMessageBus {
    ids: Arc<Mutex<AnyMap>>,
    channels: Arc<Mutex<AnyMap>>,
}

impl SharedMessageBus {
    pub fn new_handle(&self) -> Self {
        SharedMessageBus {
            ids: Arc::clone(&self.ids),
            channels: Arc::clone(&self.channels),
        }
    }

    pub fn new() -> Self {
        Self {
            ids: Arc::new(Mutex::new(AnyMap::new())),
            channels: Arc::new(Mutex::new(AnyMap::new())),
        }
    }

    //TODO: manage locks at entry level (whole map locks to generate the next id of one kind of channel)
    pub async fn next_id<M: Send + Sync + Clone + 'static>(&self) -> usize {
        self.ids
            .lock()
            .await
            .entry::<(Option<M>, AtomicUsize)>()
            .or_insert_with(|| (None, AtomicUsize::new(0)))
            .1
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn receiver<M: Send + Sync + Clone + 'static>(&self) -> broadcast::Receiver<M> {
        self.sender().await.subscribe()
    }

    pub async fn sender<M: Send + Sync + Clone + 'static>(&self) -> broadcast::Sender<M> {
        self.channels
            .lock()
            .await
            .entry::<broadcast::Sender<M>>()
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }
}

impl Default for SharedMessageBus {
    fn default() -> Self {
        Self::new()
    }

#[tokio::test]
async fn test_bus() -> anyhow::Result<()> {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    impl NeedAnswer<String> for MempoolCommand {}

    use serde::{Deserialize, Serialize};

    use crate::mempool::MempoolResponse;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CommandWithA;
    impl NeedAnswer<MempoolResponse> for CommandWithA {}
    impl NeedAnswer<String> for CommandWithA {}

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CommandWithoutA;

    use anyhow::Context;
    use command_response::{CmdRespClient, CmdRespServer, NeedAnswer};
    use listener::Listener;

    info!("Starting test");
    let bus = SharedMessageBus::new();

    let _ = &bus
        .spawn_serve(|cmd: CommandWithA| {
            info!("received gettxs command");

            Ok(Some(MempoolResponse::Txs {
                txs: vec![Transaction::default()],
            }))
        })
        .await
        .spawn_serve(|cmd: CommandWithA| {
            info!("received gettxs command");

            Ok(Some("test".to_string()))
        })
        .await
        .spawn_serve(|cmd: MempoolCommand| match cmd {
            _ => Ok(Some("".to_string())),
        })
        .await
        .spawn_listen(|_: CommandWithoutA| {
            info!("Test subscribe");
        })
        .await;

    // client request
    let resp: Option<String> = bus
        .request(MempoolCommand::GetTxs)
        .await
        .context("Requesting txs in a test")?;

    match resp.clone() {
        Some(oder) => {
            info!("Example of response reception: {} txs", oder);
        }
        None => {
            info!("Empty response");
        }
    }

    let resp: Option<MempoolResponse> = bus
        .request(CommandWithA)
        .await
        .context("Requesting txs in a test")?;

    match resp.clone() {
        Some(MempoolResponse::Txs { txs }) => {
            info!("Example of response reception: {} txs", txs.len());
        }
        _ => {
            info!("Empty response");
        }
    }

    assert_eq!(
        resp.unwrap(),
        MempoolResponse::Txs {
            txs: vec![Transaction::default()]
        }
    );

    Ok(())
}
