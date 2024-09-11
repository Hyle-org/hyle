use std::time::Duration;

use super::SharedMessageBus;
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Result;

pub const CLIENT_TIMEOUT_SECONDS: u64 = 10;

pub trait NeedAnswer<Answer> {}

#[derive(Clone)]
struct Query<Inner> {
    pub id: usize,
    pub data: Inner,
}

#[derive(Clone)]
struct QueryResponse<Inner> {
    pub id: usize,
    // anyhow err is not cloneable...
    pub data: Result<Option<Inner>, String>,
}

pub trait CmdRespClient<Cmd: NeedAnswer<Resp>, Resp> {
    async fn request(&self, cmd: Cmd) -> Result<Option<Resp>>;
}

impl<Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static, Res: Clone + Send + Sync + 'static>
    CmdRespClient<Cmd, Res> for SharedMessageBus
{
    async fn request(&self, cmd: Cmd) -> Result<Option<Res>> {
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
                // may be add timeout
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

pub trait CmdRespServer<Cmd: NeedAnswer<Resp>, Resp> {
    async fn spawn_serve(&self, routes: fn(Cmd) -> Result<Option<Resp>>) -> &Self;
}

impl<Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static, Res: Clone + Send + Sync + 'static>
    CmdRespServer<Cmd, Res> for SharedMessageBus
{
    async fn spawn_serve(&self, routes: fn(Cmd) -> Result<Option<Res>>) -> &SharedMessageBus {
        let mut receiver = self.receiver::<Query<Cmd>>().await;
        let sender = self.sender::<QueryResponse<Res>>().await;

        tokio::spawn(async move {
            loop {
                if let Ok(Query { id, data: cmd }) = receiver.recv().await {
                    _ = sender.send(QueryResponse {
                        id,
                        data: routes(cmd).map_err(|err| err.to_string()),
                    });
                }
            }
        });
        self
    }
}
