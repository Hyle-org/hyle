use anymap::{any::Any, Map};
use std::{
    borrow::BorrowMut,
    sync::{atomic::AtomicUsize, Arc},
    time::Duration,
};
use tokio::sync::{broadcast, Mutex};

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

#[tokio::test]
async fn cmd_resp_server() -> anyhow::Result<()> {
    use anyhow::Context;
    use command_response::{CmdRespClient, CmdRespServer, NeedAnswer};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CommandWithA;
    impl NeedAnswer<usize> for CommandWithA {}
    impl NeedAnswer<String> for CommandWithA {}

    let bus = SharedMessageBus::new();

    let _ = &bus
        .spawn_serve(|cmd: CommandWithA| Ok(Some(1)))
        .await
        .spawn_serve(|cmd: CommandWithA| Ok(Some("test".to_string())))
        .await;

    // client request
    let resp: Option<String> = bus
        .request(CommandWithA {})
        .await
        .context("Requesting txs in a test")?;

    assert_eq!(resp, Some("test".to_string()));

    Ok(())
}
#[tokio::test]
async fn listener() -> anyhow::Result<()> {
    use anyhow::Context;
    use listener::{Listener, Shooter};
    use serde::{Deserialize, Serialize};

    // A command type without a NeedAnswer implem
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CommandWithoutA;

    let bus = SharedMessageBus::new();

    let receipts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let shared_receipts = Arc::clone(&receipts);

    let _ = &bus
        .spawn_listen(move |cmd: CommandWithoutA| {
            let shared_receipts_clone = shared_receipts.clone();
            async move {
                shared_receipts_clone.lock().await.push("test".to_string());
            }
        })
        .await;

    // client request
    let _ = bus
        .shoot(CommandWithoutA {})
        .await
        .context("Requesting txs in a test")?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    assert!(receipts.lock().await.contains(&"test".to_string()));

    Ok(())
}
