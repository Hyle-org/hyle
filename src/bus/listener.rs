use std::future::Future;

use super::SharedMessageBus;
use anyhow::Result;

pub trait Listener<Cmd> {
    async fn spawn_listen<F, Fut>(&self, routes: F) -> &Self
    where
        F: Fn(Cmd) -> Fut + Send + 'static,
        Fut: Future + Send + 'static;
}

impl<Cmd: Clone + Send + Sync + 'static> Listener<Cmd> for SharedMessageBus {
    async fn spawn_listen<F, Fut>(&self, routes: F) -> &SharedMessageBus
    where
        F: Fn(Cmd) -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
    {
        let mut receiver = self.receiver::<Cmd>().await;

        tokio::spawn(async move {
            loop {
                if let Ok(msg) = receiver.recv().await {
                    routes(msg).await;
                }
            }
        });
        self
    }
}

pub trait Shooter<Cmd> {
    async fn shoot(&self, cmd: Cmd) -> Result<()>;
}

impl<Cmd: Clone + Send + Sync + 'static> Shooter<Cmd> for SharedMessageBus {
    async fn shoot(&self, cmd: Cmd) -> Result<()> {
        _ = self.sender().await.send(cmd);
        Ok(())
    }
}
