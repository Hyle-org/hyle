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
    ( on_bus $bus:expr, $($rest:tt)*) => {{

        handle_messages!{
            bus($bus) $($rest)*
        }

    }};

    ( bus($bus:expr) command_response<$command:ty, $response:ty> $res:pat => $handler:block $($rest:tt)*) => {{
        use paste::paste;
        use $crate::bus::command_response::*;

        // In order to generate a variable with the name server$command$response for each server
        paste! {
            let mut receiver_query_myvar = $bus.receiver::<Query<$command>>().await;
            let sender_response_myvar = $bus.sender::<QueryResponse<$response>>().await;

            handle_messages! {
                bus($bus) counter(my_var_) $($rest)*
                Ok(Query{ id, data: $res }) = receiver_query_myvar.recv() => {
                    let mut response: QueryResponse<$response> = QueryResponse {id, data: Ok(None)};
                    let res = $handler;
                    response.data = res.map_err(|err| err.to_string());
                    let _ = sender_response_myvar.send(response);
                }
            }
        }
    }};

    ( bus($bus:expr) counter($counter:ident) command_response<$command:ty,$response:ty> $res:pat => $handler:block $($rest:tt)*) => {{
        use paste::paste;
        use $crate::bus::command_response::*;

        // In order to generate a variable with the name server$command$response for each server
        paste! {
            let mut [<receiver_query_ $counter>] = $bus.receiver::<Query<$command>>().await;
            let [<sender_response_ $counter>] = $bus.sender::<QueryResponse<$response>>().await;


            handle_messages! {
                bus($bus) counter([<$counter _>]) $($rest)*
                Ok(Query{ id, data: $res }) = [<receiver_query_ $counter>].recv() => {
                    let mut response: QueryResponse<$response> = QueryResponse {id, data: Ok(None)};
                    let res = $handler;
                    response.data = res.map_err(|err| err.to_string());
                    let _ = [<sender_response_ $counter>].send(response);
                }
            }
        }
    }};


    ( bus($bus:expr) listen <$message:ty> $res:pat => $handler:block $($rest:tt)*) => {{

        // In order to generate a variable with the name server$command$response for each server
        let mut receiver_myvar = $bus.receiver::<$message>().await;

        handle_messages! {
            bus($bus) counter(myvar_) $($rest)*
            Ok($res) = receiver_myvar.recv() => {
                $handler
            }
        }
    }};

    ( bus($bus:expr) counter($counter:ident) listen <$message:ty> $res:pat => $handler:block $($rest:tt)*) => {{
        use paste::paste;
        // In order to generate a variable with the name server$command$response for each server
        paste!{
            let mut [<receiver_ $counter>] = $bus.receiver::<$message>().await;
        }

        paste! {
            handle_messages! {
                bus($bus) counter([<$counter _>]) $($rest)*
                Ok($res) = [<receiver_ $counter>].recv() => {
                    $handler
                }
            }
        }
    }};

    (counter($counter:ident) $($rest:tt)+ ) => {{
       handle_messages!($($rest)+)
    }};

    (bus($bus:expr) $($rest:tt)+ ) => {{
       handle_messages!($($rest)+)
    }};

    // Fallback to normal select cases
    ($($rest:tt)+) => {{
        loop {
            tokio::select! {
                $($rest)+
            }
        }
    }};
}

pub use handle_messages;
