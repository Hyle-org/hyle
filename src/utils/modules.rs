use std::{future::Future, pin::Pin};

use anyhow::{bail, Context, Error};
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

#[cfg(test)]
mod test {

    use super::*;

    struct ModuleA;
    impl Module for ModuleA {
        fn name() -> &'static str {
            "A"
        }
    }

    struct ModuleB;
    impl Module for ModuleB {
        fn name() -> &'static str {
            "B"
        }
    }

    struct ModuleC;
    impl Module for ModuleC {
        fn name() -> &'static str {
            "C"
        }
    }

    struct ModuleD;
    impl Module for ModuleD {
        fn name() -> &'static str {
            "D"
        }
    }

    #[tokio::test]
    async fn test_modules_start() {
        // Initialiser les modules et les ajouter à un HashMap

        let mut handler = ModulesHandler::default();
        handler.add_module::<ModuleA>(async { Ok(info!("Starting module A")) });
        handler.add_module::<ModuleC>(async { Ok(info!("Starting module C")) });
        handler.add_module::<ModuleB>(async { Ok(info!("Starting module B")) });
        handler.add_module::<ModuleD>(async { Ok(info!("Starting module D")) });

        // Démarrer les modules dans l'ordre topologique
        assert!(handler.start_modules().await.is_ok());
    }

    struct ModuleC2;
    impl Module for ModuleC2 {
        fn name() -> &'static str {
            "C"
        }
    }

    #[tokio::test]
    async fn test_modules_cycles() {
        // Initialiser les modules et les ajouter à un HashMap
        let mut handler = ModulesHandler::default();

        handler.add_module::<ModuleA>(async { Ok(info!("Starting module A")) });
        handler.add_module::<ModuleC2>(async { Ok(info!("Starting module C")) });
        handler.add_module::<ModuleB>(async { Ok(info!("Starting module B")) });
        handler.add_module::<ModuleD>(async { Ok(info!("Starting module D")) });

        assert!(handler.start_modules().await.is_err());
    }
}
