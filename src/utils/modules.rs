use std::{future::Future, pin::Pin};

use anyhow::{bail, Error, Result};
use tokio::task::JoinHandle;
use tracing::info;

/// Module trait to define startup dependencies
pub trait Module
where
    Self: Sized,
{
    type Context;

    fn name() -> &'static str;
    fn build(ctx: &Self::Context) -> impl futures::Future<Output = Result<Self>> + Send;
    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send;
}

struct ModuleStarter {
    name: &'static str,
    starter: Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'static>>,
}

impl ModuleStarter {
    fn start(self) -> Result<JoinHandle<Result<(), Error>>, std::io::Error> {
        info!("Starting module {}", self.name);
        tokio::task::Builder::new()
            .name(self.name)
            .spawn(self.starter)
    }
}

#[derive(Default)]
pub struct ModulesHandler {
    modules: Vec<ModuleStarter>,
}

impl ModulesHandler {
    async fn run_module<M>(mut module: M, ctx: M::Context) -> Result<()>
    where
        M: Module,
    {
        module.run(ctx).await
    }

    pub async fn build_module<M>(&mut self, ctx: M::Context) -> Result<()>
    where
        M: Module + 'static + Send,
        <M as Module>::Context: std::marker::Send,
    {
        let module = M::build(&ctx).await?;
        self.add_module(module, ctx)
    }

    pub fn add_module<M>(&mut self, module: M, ctx: M::Context) -> Result<()>
    where
        M: Module + 'static + Send,
        <M as Module>::Context: std::marker::Send,
    {
        self.modules.push(ModuleStarter {
            name: M::name(),
            starter: Box::pin(Self::run_module(module, ctx)),
        });
        Ok(())
    }

    /// Start Modules
    pub fn start_modules(
        &mut self,
    ) -> Result<
        (
            impl Future<Output = Result<(), Error>> + Send,
            impl FnOnce() + Send,
        ),
        Error,
    > {
        let mut tasks: Vec<JoinHandle<Result<(), Error>>> = vec![];
        let mut names: Vec<&'static str> = vec![];

        for module in self.modules.drain(..) {
            names.push(module.name);
            let handle = module.start()?;
            tasks.push(handle);
        }

        // Create an abort command (mildly hacky)
        let (tx, rx) = tokio::sync::oneshot::channel();
        let abort = move || {
            tx.send(()).ok();
        };
        tasks.push(tokio::spawn(async move {
            rx.await.ok();
            Ok(())
        }));
        names.push("abort");

        // Return a future that waits for the first error or the abort command.
        Ok((Self::wait_for_first(tasks, names), abort))
    }

    async fn wait_for_first(
        handles: Vec<JoinHandle<Result<(), Error>>>,
        names: Vec<&'static str>,
    ) -> Result<(), Error> {
        let (first, pos, remaining) = futures::future::select_all(handles).await;

        match first {
            Ok(result) => {
                // Abort remaining tasks
                for handle in remaining {
                    handle.abort();
                }
                match result {
                    Ok(_) => {
                        info!("Module {} stopped successfully", names[pos]);
                        Ok(())
                    }
                    Err(e) => bail!("Module {} stopped with error: {}", names[pos], e),
                }
            }
            Err(e) => anyhow::bail!("Error while waiting for first module: {}", e),
        }
    }
}
