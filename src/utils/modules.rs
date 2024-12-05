use std::{any::type_name, fs, future::Future, path::Path, pin::Pin, time::Duration};

use anyhow::{anyhow, Context, Error, Result};
use rand::{distributions::Alphanumeric, Rng};
use signal::ShutdownCompleted;
use tokio::task::JoinHandle;
use tracing::{debug, info};

use crate::{bus::SharedMessageBus, handle_messages, utils::logger::LogMe};

/// Module trait to define startup dependencies
pub trait Module
where
    Self: Sized,
{
    type Context;

    fn name() -> &'static str;
    fn build(ctx: Self::Context) -> impl futures::Future<Output = Result<Self>> + Send;
    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send;

    fn load_from_disk<S>(file: &Path) -> Option<S>
    where
        S: bincode::Decode,
    {
        info!("Loading file {}", file.to_string_lossy());
        match fs::File::open(file) {
            Ok(mut reader) => {
                bincode::decode_from_std_read(&mut reader, bincode::config::standard())
                    .log_error(format!("Loading and decoding {}", file.to_string_lossy()))
                    .ok()
            }
            Err(e) => {
                info!(
                    "File {} not found for module {} (using default): {:?}",
                    file.to_string_lossy(),
                    type_name::<S>(),
                    e
                );
                None
            }
        }
    }

    fn load_from_disk_or_default<S>(file: &Path) -> S
    where
        S: bincode::Decode + Default,
    {
        Self::load_from_disk(file).unwrap_or(S::default())
    }

    fn save_on_disk<S>(folder: &Path, file: &Path, store: &S) -> Result<()>
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
        let tmp = folder.join(tmp.clone());
        let mut writer = fs::File::create(tmp.as_path()).log_error("Create file")?;
        bincode::encode_into_std_write(store, &mut writer, bincode::config::standard())
            .log_error("Serializing Ctx chain")?;
        fs::rename(tmp, file).log_error("Rename file")?;
        Ok(())
    }
}

struct ModuleStarter {
    pub name: &'static str,
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

pub mod signal {
    use crate::bus::BusMessage;
    #[derive(Clone, Debug)]
    pub struct ShutdownModule {
        pub module: String,
    }
    #[derive(Clone, Debug)]
    pub struct ShutdownCompleted {
        pub module: String,
    }

    impl BusMessage for ShutdownModule {}
    impl BusMessage for ShutdownCompleted {}
}

macro_rules! module_bus_client {
    (
        $(#[$meta:meta])*
        $pub:vis struct $name:ident {
            $(module: $module:ty,)?
            $(sender($sender:ty),)*
            $(receiver($receiver:ty),)*
        }
    ) => {
        $crate::bus::bus_client!{
            $(#[$meta])*
            $pub struct $name {
                sender($crate::utils::modules::signal::ShutdownCompleted),
                sender($crate::utils::modules::signal::ShutdownModule),
                $(sender($sender),)*
                $(receiver($receiver),)*
                receiver($crate::utils::modules::signal::ShutdownCompleted),
                receiver($crate::utils::modules::signal::ShutdownModule),
            }
        }

        impl $name {
            #[allow(unused)]
             pub fn shutdown(&mut self, module_name: String) -> Result<()> {
                _ = self.send($crate::utils::modules::signal::ShutdownModule { module: module_name })?;
                Ok(())
            }
            $(
            #[allow(unused)]
             pub fn shutdown_complete(&mut self) -> Result<()> {
                _ = self.send($crate::utils::modules::signal::ShutdownCompleted { module: stringify!($module).to_string() })?;
                Ok(())
            }
            )?
        }
    }
}

pub(crate) use module_bus_client;

module_bus_client! {
    pub struct ShutdownClient {}
}

impl ShutdownClient {
    pub async fn shutdown_module(&mut self, module_name: &str) {
        _ = self
            .shutdown(module_name.to_string())
            .log_error("Shutting down module");

        handle_messages! {
            on_bus *self,
            listen<ShutdownCompleted> msg => {
                if msg.module == module_name {
                    info!("Module {} successfully shut", msg.module);
                    break;
                }
            }
        }
    }
}

pub struct ModulesHandler {
    bus: ShutdownClient,
    modules: Vec<ModuleStarter>,
    started_modules: Vec<&'static str>,
}

impl ModulesHandler {
    pub async fn new(shared_bus: &SharedMessageBus) -> ModulesHandler {
        let shutdown_client = ShutdownClient::new_from_bus(shared_bus.new_handle()).await;

        ModulesHandler {
            bus: shutdown_client,
            modules: vec![],
            started_modules: vec![],
        }
    }

    pub async fn start_modules(&mut self) -> Result<()> {
        let mut tasks: Vec<JoinHandle<Result<()>>> = vec![];

        for module in self.modules.drain(..) {
            self.started_modules.push(module.name);
            let handle = module.start()?;
            tasks.push(handle);
        }

        // Return a future that waits for the first error or the abort command.
        futures::future::select_all(tasks)
            .await
            .0
            .context("Joining error")?
    }

    /// Shutdown modules in reverse order (start A, B, C, shutdown C, B, A)
    pub async fn shutdown_modules(&mut self, timeout: Duration) -> Result<()> {
        for module_name in self.started_modules.drain(..).rev() {
            if !["Genesis"].contains(&module_name) {
                _ = tokio::time::timeout(timeout, self.bus.shutdown_module(module_name))
                    .await
                    .log_error(format!("Shutting down module {module_name}"));
            }
        }

        Ok(())
    }

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
}

#[cfg(test)]
mod tests {
    use crate::handle_messages;

    use super::*;
    use crate::bus::SharedMessageBus;
    use signal::ShutdownModule;
    use std::fs::File;
    use tempfile::tempdir;

    #[derive(Default, bincode::Encode, bincode::Decode)]
    struct TestStruct {
        value: u32,
    }

    struct TestModule {
        bus: TestBusClient,
    }

    module_bus_client! {
        struct TestBusClient {
            module: TestModule,
        }
    }

    impl Module for TestModule {
        type Context = TestBusClient;

        fn name() -> &'static str {
            "TestModule"
        }

        async fn build(_ctx: Self::Context) -> Result<Self> {
            Ok(TestModule { bus: _ctx })
        }

        async fn run(&mut self) -> Result<()> {
            handle_messages! {
                on_bus self.bus,
                break_on(stringify!(TestModule))
            }

            _ = self.bus.send(ShutdownCompleted {
                module: stringify!(TestModule).to_string(),
            });
            Ok(())
        }
    }

    #[test]
    fn test_load_from_disk_or_default() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file");

        // Write a valid TestStruct to the file
        let mut file = File::create(&file_path).unwrap();
        let test_struct = TestStruct { value: 42 };
        bincode::encode_into_std_write(&test_struct, &mut file, bincode::config::standard())
            .unwrap();

        // Load the struct from the file
        let loaded_struct: TestStruct = TestModule::load_from_disk_or_default(&file_path);
        assert_eq!(loaded_struct.value, 42);

        // Load from a non-existent file
        let non_existent_path = dir.path().join("non_existent_file");
        let default_struct: TestStruct = TestModule::load_from_disk_or_default(&non_existent_path);
        assert_eq!(default_struct.value, 0);
    }

    #[test]
    fn test_save_on_disk() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file");

        let test_struct = TestStruct { value: 42 };
        TestModule::save_on_disk(dir.path(), &file_path, &test_struct).unwrap();

        // Load the struct from the file to verify it was saved correctly
        let loaded_struct: TestStruct = TestModule::load_from_disk_or_default(&file_path);
        assert_eq!(loaded_struct.value, 42);
    }

    #[tokio::test]
    async fn test_build_module() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut handler = ModulesHandler::new(&shared_bus).await;
        handler
            .build_module::<TestModule>(TestBusClient::new_from_bus(shared_bus.new_handle()).await)
            .await
            .unwrap();
        assert_eq!(handler.modules.len(), 1);
    }

    #[tokio::test]
    async fn test_add_module() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut handler = ModulesHandler::new(&shared_bus).await;
        let module = TestModule {
            bus: TestBusClient::new_from_bus(shared_bus.new_handle()).await,
        };

        handler.add_module(module).unwrap();
        assert_eq!(handler.modules.len(), 1);
    }

    async fn is_future_pending<F: Future>(future: F) -> bool {
        tokio::select! {
            _ = future => false, // La future est prête
            _ = tokio::time::sleep(Duration::from_millis(1)) => true, // Timeout, donc la future est pending
        }
    }

    #[tokio::test]
    async fn test_start_modules() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut shutdown_receiver = get_receiver::<ShutdownModule>(&shared_bus).await;
        let mut shutdown_completed_receiver = get_receiver::<ShutdownCompleted>(&shared_bus).await;
        let mut handler = ModulesHandler::new(&shared_bus).await;
        handler
            .build_module::<TestModule>(TestBusClient::new_from_bus(shared_bus.new_handle()).await)
            .await
            .unwrap();
        let handle = handler.start_modules();

        assert!(is_future_pending(handle).await);

        _ = handler.shutdown_modules(Duration::from_secs(1)).await;

        assert_eq!(
            shutdown_receiver.recv().await.unwrap().module,
            "TestModule".to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            "TestModule".to_string()
        );
    }
}
