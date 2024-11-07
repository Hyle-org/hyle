use std::{fs, future::Future, path::Path, pin::Pin};

use anyhow::{bail, Error, Result};
use rand::{distributions::Alphanumeric, Rng};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::utils::logger::LogMe;

/// Module trait to define startup dependencies
pub trait Module
where
    Self: Sized,
{
    type Context;

    fn name() -> &'static str;
    fn build(ctx: Self::Context) -> impl futures::Future<Output = Result<Self>> + Send;
    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send;

    fn load_from_disk_or_default<S>(file: &Path) -> S
    where
        S: bincode::Decode + Default,
    {
        fs::File::open(file)
            .map_err(|e| e.to_string())
            .and_then(|mut reader| {
                bincode::decode_from_std_read(&mut reader, bincode::config::standard())
                    .map_err(|e| e.to_string())
            })
            .unwrap_or_else(|e| {
                warn!(
                    "{}: Failed to load data from disk ({}). Error was: {e}",
                    Self::name(),
                    file.display()
                );
                S::default()
            })
    }

    fn save_on_disk<S>(file: &Path, store: &S) -> Result<()>
    where
        S: bincode::Encode,
    {
        // TODO/FIXME: Concurrent writes can happen, and an older state can override a newer one
        // Example:
        // State 1 starts creating a tmp file data.state1.tmp
        // State 2 starts creating a tmp file data.state2.tmp
        // rename data.state2.tmp into store (atomic override)
        // renemae data.state1.tmp into
        let salt: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(8)
            .map(char::from)
            .collect();
        let tmp = format!("{}.{}.data.tmp", salt, Self::name());
        debug!("Saving on disk in a tmp file {}", tmp.clone());
        let mut writer = fs::File::create(tmp.clone()).log_error("Create file")?;
        bincode::encode_into_std_write(store, &mut writer, bincode::config::standard())
            .log_error("Serializing Ctx chain")?;
        fs::rename(tmp, file).log_error("Rename file")?;
        Ok(())
    }
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
    async fn run_module<M>(mut module: M) -> Result<()>
    where
        M: Module,
    {
        module.run().await
    }

    pub async fn build_module<M>(&mut self, ctx: M::Context) -> Result<()>
    where
        M: Module + 'static + Send,
        <M as Module>::Context: std::marker::Send,
    {
        let module = M::build(ctx).await?;
        self.add_module(module)
    }

    pub fn add_module<M>(&mut self, module: M) -> Result<()>
    where
        M: Module + 'static + Send,
        <M as Module>::Context: std::marker::Send,
    {
        self.modules.push(ModuleStarter {
            name: M::name(),
            starter: Box::pin(Self::run_module(module)),
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
        mut handles: Vec<JoinHandle<Result<(), Error>>>,
        names: Vec<&'static str>,
    ) -> Result<(), Error> {
        while !handles.is_empty() {
            let (first, pos, remaining) = futures::future::select_all(handles).await;
            handles = remaining;

            match first {
                Ok(result) => match result {
                    Ok(_) => {
                        info!("Module {} stopped successfully", names[pos]);
                    }
                    Err(e) => {
                        error!("Module {} stopped with error: {}", names[pos], e);
                        // Abort remaining tasks
                        for handle in handles {
                            handle.abort();
                        }
                        bail!("Error in module {}", names[pos]);
                    }
                },
                Err(e) => {
                    bail!("Error while waiting for module {}: {}", names[pos], e)
                }
            }
        }
        Ok(())
    }
}
