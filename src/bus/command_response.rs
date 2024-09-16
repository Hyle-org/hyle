use std::time::Duration;

use crate::mempool::MempoolCommand;
use crate::mempool::MempoolResponse;

use super::SharedMessageBus;
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Result;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::Sender;
use tracing::info;

pub const CLIENT_TIMEOUT_SECONDS: u64 = 10;

pub trait NeedAnswer<Answer> {}

#[derive(Clone)]
pub struct Query<Inner> {
    pub id: usize,
    pub data: Inner,
}

#[derive(Clone)]
pub struct QueryResponse<Inner> {
    pub id: usize,
    // anyhow err is not cloneable...
    pub data: Result<Option<Inner>, String>,
}

pub trait CmdRespClient {
    fn request<
        Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static,
        Res: Clone + Send + Sync + 'static,
    >(
        &self,
        cmd: Cmd,
    ) -> impl std::future::Future<Output = Result<Option<Res>>> + Send;
}

impl CmdRespClient for SharedMessageBus {
    async fn request<
        Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static,
        Res: Clone + Send + Sync + 'static,
    >(
        &self,
        cmd: Cmd,
    ) -> Result<Option<Res>> {
        //TODO: Reminder/whatever: there is a lock on the whole counters map
        let next_id = self.next_id::<Cmd>().await;

        let query_cmd = Query {
            id: next_id,
            data: cmd,
        };

        let mut receiver = self.receiver::<QueryResponse<Res>>().await;

        _ = self.sender().await.send(query_cmd);

        match tokio::time::timeout(Duration::from_secs(CLIENT_TIMEOUT_SECONDS), async move {
            loop {
                if let Ok(resp) = receiver.recv().await {
                    if resp.id == next_id {
                        return resp.data.map_err(|err_str| anyhow!(err_str));
                    }
                }
            }
        })
        .await
        {
            Ok(res) => res,
            Err(timeouterror) => bail!(
                "Timeout triggered while calling topic with query: {}",
                timeouterror.to_string()
            ),
        }
    }
}

pub struct CommandResponseServer<Cmd: NeedAnswer<Res>, Res> {
    receiver: Receiver<Query<Cmd>>,
    sender: Sender<QueryResponse<Res>>,
}

pub trait CommandResponseServerCreate {
    fn create_server<
        Cmd: NeedAnswer<Resp> + Clone + Send + Sync + 'static,
        Resp: Clone + Send + Sync + 'static,
    >(
        &self,
    ) -> impl std::future::Future<Output = CommandResponseServer<Cmd, Resp>>;
}

impl CommandResponseServerCreate for SharedMessageBus {
    async fn create_server<
        Cmd: NeedAnswer<Resp> + Clone + Send + Sync + 'static,
        Resp: Clone + Send + Sync + 'static,
    >(
        &self,
    ) -> CommandResponseServer<Cmd, Resp> {
        CommandResponseServer {
            receiver: self.receiver().await,
            sender: self.sender().await,
        }
    }
}

impl<Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static, Res: Clone + Send + Sync + 'static>
    CommandResponseServer<Cmd, Res>
{
    pub async fn get_query(&mut self) -> Result<Query<Cmd>, RecvError> {
        self.receiver.recv().await
    }

    pub fn respond(&self, res: QueryResponse<Res>) -> Result<()> {
        let _ = self.sender.send(res);

        Ok(())
    }

    pub fn to_response(&self, Query { id, data }: Query<Cmd>) -> (Cmd, QueryResponse<Res>) {
        (data, QueryResponse { id, data: Ok(None) })
    }
}

impl<Res: Clone + Send + Sync + 'static> QueryResponse<Res> {
    pub fn updated(self, res: Result<Option<Res>>) -> QueryResponse<Res> {
        QueryResponse {
            id: self.id,
            data: res.map_err(|err| err.to_string()),
        }
    }
}

#[macro_use]
macro_rules! my_select {

    (command_response($server:expr) = $res:ident => $handler:block, $($rest:tt)*) => {{
        tokio::select! {
            Ok(query) = $server.get_query() => {
                let ($res, response_writer) = $server.to_response(query);
                let res = $handler;
                let _ = $server.respond(response_writer.updated(res));
            }

            $($rest)*
        }
    }};


    // Match timeout case with specific duration
    (timeout($duration:expr) => $timeout_block:block, $($rest:tt)*) => {{
        use tokio::time::{sleep, Duration};
        tokio::select! {
            _ = sleep(Duration::from_millis($duration)) => $timeout_block,
            $($rest)*
        }
    }};

    // Fallback to normal select cases
    ($($rest:tt)*) => {{
        tokio::select! {
            $($rest)*
        }
    }};



}

async fn test() {
    let mut server = SharedMessageBus::new()
        .create_server::<MempoolCommand, MempoolResponse>()
        .await;

    let mut receiver = SharedMessageBus::new().receiver::<MempoolCommand>().await;

    my_select! {

        command_response(server) = cmd => {
            info!("{:?}", cmd);
            Ok(Some(MempoolResponse::PendingBatch { id: 2.to_string(), txs: vec![] }))
        },

        Ok(q) = receiver.recv() => {

        }
    };
}
