use std::time::Duration;

use super::SharedMessageBus;
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Result;

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

#[macro_export]
macro_rules! handle_messages {
    ( $(command_response <$command:ident,$response:ident>($bus:expr) = $res:ident => $handler:block)+, $($rest:tt)*) => {{
        use paste::paste;
        use $crate::bus::command_response::*;

        $(
            // In order to generate a variable with the name server$command$response for each server
            paste! {
                let mut [<receiver_query_ $command:lower>] = $bus.receiver::<Query<$command>>().await;
                let [<sender_response_ $response:lower>] = $bus.sender::<QueryResponse<$response>>().await;
            }
        )+

        handle_messages! {
            $($rest)*
            $(
                Ok(Query{ id, data: $res }) = paste!([<receiver_query_ $command:lower>]).recv() => {
                    let mut response: QueryResponse<$response> = QueryResponse {id, data: Ok(None)};
                    let res = $handler;
                    response.data = res.map_err(|err| err.to_string());
                    let _ = paste!([<sender_response_ $response:lower>]).send(response);
                }
            )+
        }
    }};

    ( $(listen <$message:ident>($bus:expr) = $res:ident => $handler:block)+, $($rest:tt)*) => {{
        use paste::paste;

        $(
            // In order to generate a variable with the name server$command$response for each server
            paste! {
                let mut [<receiver_ $message:lower>] = $bus.receiver::<$message>().await;
            }
        )+

        handle_messages! {
            $($rest)*
            $(
                Ok($res) = paste!([<receiver_ $message:lower>]).recv() => {
                    $handler
                }
            )+
        }
    }};

    // Fallback to normal select cases
    ($($rest:tt)*) => {{
        loop {
            tokio::select! {
                $($rest)*
            }
        }
    }};
}

pub use handle_messages;
