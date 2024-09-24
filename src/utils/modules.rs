use std::{future::Future, pin::Pin};

use anyhow::{bail, Error};
use tokio::task::JoinHandle;
use tracing::info;

/// Module trait to define startup dependencies
pub trait Module {
    fn name() -> &'static str;
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
    pub fn add_module<M>(
        &mut self,
        starter: impl Future<Output = Result<(), Error>> + Send + 'static,
    ) where
        M: Module,
    {
        self.modules.push(ModuleStarter {
            name: M::name(),
            starter: Box::pin(starter),
        });
    }

    /// Start Modules
    pub async fn start_modules(&mut self) -> Result<(), Error> {
        let mut tasks: Vec<JoinHandle<Result<(), Error>>> = vec![];
        let mut names: Vec<&'static str> = vec![];

        for module in self.modules.drain(..) {
            names.push(module.name);
            let handle = module.start()?;
            tasks.push(handle);
        }

        // Wait for the first task to finish
        Self::wait_for_first(tasks, names).await
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
