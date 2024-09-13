use std::future::Future;
use std::time::Duration;

use super::SharedMessageBus;
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Result;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::Sender;

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
    async fn request<
        Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static,
        Res: Clone + Send + Sync + 'static,
    >(
        &self,
        cmd: Cmd,
    ) -> Result<Option<Res>>;
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

#[macro_export]
macro_rules! handle_server_query {
    ($server:expr, $query:expr, $self:ident, $handler:ident) => {
        let (cmd, response_writer) = $server.to_response($query);
        let res = $self.$handler(cmd);
        let _ = $server.respond(response_writer.updated(res));
    };
}
pub struct CommandResponseServer<Cmd: NeedAnswer<Res>, Res> {
    receiver: Receiver<Query<Cmd>>,
    sender: Sender<QueryResponse<Res>>,
}

pub trait CommandResponseServerCreate {
    async fn create_server<
        Cmd: NeedAnswer<Resp> + Clone + Send + Sync + 'static,
        Resp: Clone + Send + Sync + 'static,
    >(
        &self,
    ) -> CommandResponseServer<Cmd, Resp>;
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
    // pub async fn respond<F, Fut>(&self, Query { id, data }: Query<Cmd>, f: F) -> Result<()>
    // where
    //     F: Fn(Cmd) -> Fut + Send + 'static,
    //     Fut: Future<Output = Result<Option<Res>>> + Send + 'static,
    // {
    //     let sent = self.sender.send(QueryResponse {
    //         id,
    //         data: f(data).await.map_err(|err_anyhow| err_anyhow.to_string()),
    //     });

    //     match sent {
    //         Ok(_) => Ok(()),
    //         Err(e) => {
    //             bail!(
    //                 "Sending message through response channel: {}",
    //                 e.to_string()
    //             )
    //         }
    //     }
    // }

    // pub async fn respond_sync<F>(&self, Query { id, data }: Query<Cmd>, f: F) -> Result<()>
    // where
    //     F: Fn(Cmd) -> Result<Option<Res>> + Send + 'static,
    // {
    //     let sent = self.sender.send(QueryResponse {
    //         id,
    //         data: f(data).map_err(|err_anyhow| err_anyhow.to_string()),
    //     });

    //     match sent {
    //         Ok(_) => Ok(()),
    //         Err(e) => {
    //             bail!(
    //                 "Sending message through response channel: {}",
    //                 e.to_string()
    //             )
    //         }
    //     }
    // }

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

// pub trait CmdRespAsyncServer<Cmd: NeedAnswer<Resp>, Resp> {
//     async fn serve_async<F, Fut>(&self, routes: F) -> &Self
//     where
//         F: Fn(Cmd) -> Fut + Send + 'static,
//         Fut: Future<Output = Result<Option<Resp>>> + Send + 'static;
// }

// impl<Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static, Res: Clone + Send + Sync + 'static>
//     CmdRespAsyncServer<Cmd, Res> for SharedMessageBus
// {
//     async fn serve_async<F, Fut>(&self, routes: F) -> &SharedMessageBus
//     where
//         F: Fn(Cmd) -> Fut + Send + 'static,
//         Fut: Future<Output = Result<Option<Res>>> + Send + 'static,
//     {
//         let mut receiver = self.receiver::<Query<Cmd>>().await;
//         let sender = self.sender::<QueryResponse<Res>>().await;

//         tokio::spawn(async move {
//             loop {
//                 if let Ok(Query { id, data: cmd }) = receiver.recv().await {
//                     _ = sender.send(QueryResponse {
//                         id,
//                         data: routes(cmd).await.map_err(|err| err.to_string()),
//                     });
//                 }
//             }
//         });
//         self
//     }
// }

// pub trait CmdRespSyncServer<Cmd: NeedAnswer<Resp>, Resp> {
//     async fn serve_sync(
//         &self,
//         routes: impl Fn(Cmd) -> Result<Option<Resp>> + Send + 'static,
//     ) -> &Self;
// }

// impl<Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static, Res: Clone + Send + Sync + 'static>
//     CmdRespSyncServer<Cmd, Res> for SharedMessageBus
// {
//     async fn serve_sync(
//         &self,
//         routes: impl Fn(Cmd) -> Result<Option<Res>> + Send + 'static,
//     ) -> &SharedMessageBus {
//         let mut receiver = self.receiver::<Query<Cmd>>().await;
//         let sender = self.sender::<QueryResponse<Res>>().await;

//         tokio::spawn(async move {
//             loop {
//                 if let Ok(Query { id, data: cmd }) = receiver.recv().await {
//                     _ = sender.send(QueryResponse {
//                         id,
//                         data: routes(cmd).map_err(|err| err.to_string()),
//                     });
//                 }
//             }
//         });
//         self
//     }
// }
