use std::{
    any::type_name,
    fs,
    future::Future,
    io::{BufWriter, Write},
    path::Path,
    pin::Pin,
    time::Duration,
};

use crate::{
    bus::{bus_client, BusClientSender, SharedMessageBus},
    genesis::Genesis,
    handle_messages,
    utils::logger::LogMe,
};
use anyhow::{bail, Error, Result};
use futures::future::select_all;
use rand::{distr::Alphanumeric, Rng};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace};

/// Module trait to define startup dependencies
pub trait Module
where
    Self: Sized,
{
    type Context;

    fn build(ctx: Self::Context) -> impl futures::Future<Output = Result<Self>> + Send;
    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send;

    fn load_from_disk<S>(file: &Path) -> Option<S>
    where
        S: borsh::BorshDeserialize,
    {
        match fs::File::open(file) {
            Ok(mut reader) => {
                info!("Loaded data from disk {}", file.to_string_lossy());
                borsh::from_reader(&mut reader)
                    .log_error(format!("Loading and decoding {}", file.to_string_lossy()))
                    .ok()
            }
            Err(_) => {
                info!(
                    "File {} not found for module {} (using default)",
                    file.to_string_lossy(),
                    type_name::<S>(),
                );
                None
            }
        }
    }

    fn load_from_disk_or_default<S>(file: &Path) -> S
    where
        S: borsh::BorshDeserialize + Default,
    {
        Self::load_from_disk(file).unwrap_or(S::default())
    }

    fn save_on_disk<S>(file: &Path, store: &S) -> Result<()>
    where
        S: borsh::BorshSerialize,
    {
        // TODO/FIXME: Concurrent writes can happen, and an older state can override a newer one
        // Example:
        // State 1 starts creating a tmp file data.state1.tmp
        // State 2 starts creating a tmp file data.state2.tmp
        // rename data.state2.tmp into store (atomic override)
        // renemae data.state1.tmp into
        let salt: String = rand::rng()
            .sample_iter(&Alphanumeric)
            .take(8)
            .map(char::from)
            .collect();
        let tmp = file.with_extension(format!("{}.tmp", salt));
        debug!("Saving on disk in a tmp file {:?}", tmp.clone());
        let mut buf_writer =
            BufWriter::new(fs::File::create(tmp.as_path()).log_error("Create file")?);
        borsh::to_writer(&mut buf_writer, store).log_error("Serializing Ctx chain")?;

        buf_writer.flush().log_error(format!(
            "Flushing Buffer writer for store {}",
            type_name::<S>()
        ))?;
        debug!("Renaming {:?} to {:?}", &tmp, &file);
        fs::rename(tmp, file).log_error("Rename file")?;
        Ok(())
    }
}

struct ModuleStarter {
    pub name: &'static str,
    starter: Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'static>>,
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

    pub async fn async_receive_shutdown<T>(
        should_shutdown: &mut bool,
        shutdown_receiver: &mut tokio::sync::broadcast::Receiver<
            crate::utils::modules::signal::ShutdownModule,
        >,
    ) -> anyhow::Result<()> {
        if *should_shutdown {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            return Ok(());
        }
        while let Ok(shutdown_event) = shutdown_receiver.recv().await {
            if shutdown_event.module == std::any::type_name::<T>() {
                tracing::debug!(
                    "Break signal received for module {}",
                    std::any::type_name::<T>()
                );
                *should_shutdown = true;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                return Ok(());
            }
        }
        anyhow::bail!(
            "Error while shutting down module {}",
            std::any::type_name::<T>()
        );
    }
}

#[macro_export]
macro_rules! module_handle_messages {
    (on_bus $bus:expr, delay_shutdown_until  $lay_shutdow_until:block, $($rest:tt)*) => {
        {
            let mut shutdown_receiver = unsafe { &mut *Pick::<tokio::sync::broadcast::Receiver<$crate::utils::modules::signal::ShutdownModule>>::splitting_get_mut(&mut $bus) };
            let mut should_shutdown = false;
            $crate::handle_messages! {
                on_bus $bus,
                $($rest)*
                Ok(_) = $crate::utils::modules::signal::async_receive_shutdown::<Self>(&mut should_shutdown, &mut shutdown_receiver) => {
                    let res = $lay_shutdow_until;
                    if res {
                        break;
                    }
                }
            }
            should_shutdown
        }
    };
    (on_bus $bus:expr, $($rest:tt)*) => {
        {
            let mut shutdown_receiver = unsafe { &mut *Pick::<tokio::sync::broadcast::Receiver<$crate::utils::modules::signal::ShutdownModule>>::splitting_get_mut(&mut $bus) };
            let mut should_shutdown = false;
            $crate::handle_messages! {
                on_bus $bus,
                $($rest)*
                Ok(_) = $crate::utils::modules::signal::async_receive_shutdown::<Self>(&mut should_shutdown, &mut shutdown_receiver) => {
                    break;
                }
            }
            should_shutdown
        }
    };
}

#[macro_export]
macro_rules! module_bus_client {
    (
        $(#[$meta:meta])*
        $pub:vis struct $name:ident {
            $(sender($sender:ty),)*
            $(receiver($receiver:ty),)*
        }
    ) => {
        $crate::bus::bus_client!{
            $(#[$meta])*
            $pub struct $name {
                $(sender($sender),)*
                $(receiver($receiver),)*
                receiver($crate::utils::modules::signal::ShutdownModule),
            }
        }
    }
}

pub use module_bus_client;

bus_client! {
    pub struct ShutdownClient {
        sender(signal::ShutdownModule),
        sender(signal::ShutdownCompleted),
        receiver(signal::ShutdownCompleted),
    }
}

pub struct ModulesHandler {
    bus: SharedMessageBus,
    modules: Vec<ModuleStarter>,
    started_modules: Vec<&'static str>,
    running_modules: Vec<JoinHandle<()>>,
    shut_modules: Vec<String>,
}

impl ModulesHandler {
    pub async fn new(shared_bus: &SharedMessageBus) -> ModulesHandler {
        let shared_message_bus = shared_bus.new_handle();

        ModulesHandler {
            bus: shared_message_bus,
            modules: vec![],
            started_modules: vec![],
            running_modules: vec![],
            shut_modules: vec![],
        }
    }

    fn long_running_module(module_name: &str) -> bool {
        ![std::any::type_name::<Genesis>()].contains(&module_name)
    }

    pub async fn start_modules(&mut self) -> Result<()> {
        if !self.running_modules.is_empty() {
            bail!("Modules are already running!");
        }

        for module in self.modules.drain(..) {
            if Self::long_running_module(module.name) {
                self.started_modules.push(module.name);
            }

            debug!("Starting module {}", module.name);

            let mut shutdown_client = ShutdownClient::new_from_bus(self.bus.new_handle()).await;
            let task = tokio::task::Builder::new()
                .name(module.name)
                .spawn(async move {
                    match module.starter.await {
                        Ok(_) => tracing::debug!("Module {} exited with no error.", module.name),
                        Err(e) => {
                            tracing::error!("Module {} exited with error: {:?}", module.name, e);
                        }
                    }
                    _ = shutdown_client
                        .send(signal::ShutdownCompleted {
                            module: module.name.to_string(),
                        })
                        .log_error("Sending ShutdownCompleted message");
                })?;

            if Self::long_running_module(module.name) {
                self.running_modules.push(task);
            }
        }

        Ok(())
    }

    pub async fn shutdown_loop(&mut self) -> Result<()> {
        if self.started_modules.is_empty() {
            return Ok(());
        }

        let mut shutdown_client = ShutdownClient::new_from_bus(self.bus.new_handle()).await;

        // Sends a trigger event when one task ends (should not, but in case of panic, no event is sent)
        let join_set: Vec<JoinHandle<()>> = self.running_modules.drain(..).collect();
        let started_modules_cloned = self.started_modules.clone();
        tokio::task::Builder::new()
            .name("module-panic-failure-listener")
            .spawn(async move {
                trace!("Module failure listener - Join set size {}", join_set.len());
                trace!(
                    "Module failure listener - Started modules {:?}",
                    started_modules_cloned.clone()
                );
                if join_set.is_empty() {
                    return;
                }
                let (_res, idx, _remaining) = select_all(join_set).await;
                if let Some(module_name) = started_modules_cloned.get(idx) {
                    debug!("First module to shutdown {}", module_name);

                    _ = shutdown_client
                        .send(signal::ShutdownCompleted {
                            module: module_name.to_string(),
                        })
                        .log_error("Sending ShutdownCompleted message");
                }
            })?;

        let mut shutdown_client = ShutdownClient::new_from_bus(self.bus.new_handle()).await;
        // Return a future that waits for the first error or the abort command.
        handle_messages! {
            on_bus shutdown_client,
            listen<signal::ShutdownCompleted> msg => {
                if Self::long_running_module(msg.module.as_str()) && !self.shut_modules.contains(&msg.module)  {
                    self.started_modules.retain(|module| *module != msg.module.clone());
                    self.shut_modules.push(msg.module.clone());
                    if self.started_modules.is_empty() {
                        break;
                    } else {
                        _ = self.shutdown_next_module().await;
                    }
                }
            }

            _ = tokio::time::sleep(Duration::from_secs(3)) => {
                if !self.shut_modules.is_empty() {
                    _ = self.shutdown_next_module().await;
                }
            }
        }

        Ok(())
    }

    /// Shutdown modules in reverse order (start A, B, C, shutdown C, B, A)
    pub async fn shutdown_next_module(&mut self) -> Result<()> {
        let mut shutdown_client = ShutdownClient::new_from_bus(self.bus.new_handle()).await;
        if let Some(module_name) = self.started_modules.pop() {
            // May be the shutdown message was skipped because the module failed somehow
            if !self.shut_modules.contains(&module_name.to_string()) {
                _ = shutdown_client
                    .send(signal::ShutdownModule {
                        module: module_name.to_string(),
                    })
                    .log_error("Shutting down module");
            } else {
                tracing::debug!("Not shutting already shut module {}", module_name);
            }
        }

        Ok(())
    }

    pub async fn shutdown_modules(&mut self) -> Result<()> {
        self.shutdown_next_module().await?;
        self.shutdown_loop().await?;

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
            name: type_name::<M>(),
            starter: Box::pin(Self::run_module(module)),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::bus::{dont_use_this::get_receiver, metrics::BusMetrics, BusMessage};

    use super::*;
    use crate::bus::SharedMessageBus;
    use signal::{ShutdownCompleted, ShutdownModule};
    use std::{fs::File, sync::Arc};
    use tempfile::tempdir;
    use tokio::sync::Mutex;

    #[derive(Default, borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct TestStruct {
        value: u32,
    }

    struct TestModule<T> {
        bus: TestBusClient,
        _field: T,
    }

    impl BusMessage for usize {}

    module_bus_client! {
        struct TestBusClient { sender(usize), }
    }

    macro_rules! test_module {
        ($bus_client:ty, $tag:ty) => {
            impl Module for TestModule<$tag> {
                type Context = $bus_client;
                async fn build(_ctx: Self::Context) -> Result<Self> {
                    Ok(TestModule {
                        bus: _ctx,
                        _field: Default::default(),
                    })
                }

                async fn run(&mut self) -> Result<()> {
                    let nb_shutdowns: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
                    let cloned = Arc::clone(&nb_shutdowns);
                    module_handle_messages! {
                        on_bus self.bus,
                        _ = async {
                            let mut guard = cloned.lock().await;
                            (*guard) += 1;
                            std::future::pending::<()>().await
                        } => { }
                    };

                    self.bus.send(*cloned.lock().await).expect(
                        "Error while sending the number of loop cancellations while shutting down",
                    );

                    Ok(())
                }
            }
        };
    }

    test_module!(TestBusClient, String);
    test_module!(TestBusClient, usize);
    test_module!(TestBusClient, bool);

    // Failing module by breaking event loop

    impl Module for TestModule<u64> {
        type Context = TestBusClient;
        async fn build(_ctx: Self::Context) -> Result<Self> {
            Ok(TestModule {
                bus: _ctx,
                _field: Default::default(),
            })
        }

        async fn run(&mut self) -> Result<()> {
            module_handle_messages! {
                on_bus self.bus,
                _ = async { } => {
                    break;
                }
            };

            Ok(())
        }
    }

    // Failing module by early exit (no shutdown completed event emitted)

    impl Module for TestModule<u32> {
        type Context = TestBusClient;
        async fn build(_ctx: Self::Context) -> Result<Self> {
            Ok(TestModule {
                bus: _ctx,
                _field: Default::default(),
            })
        }

        async fn run(&mut self) -> Result<()> {
            panic!("bruh");
        }
    }

    #[test]
    fn test_load_from_disk_or_default() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file");

        // Write a valid TestStruct to the file
        let mut file = File::create(&file_path).unwrap();
        let test_struct = TestStruct { value: 42 };
        borsh::to_writer(&mut file, &test_struct).unwrap();

        // Load the struct from the file
        let loaded_struct: TestStruct = TestModule::<usize>::load_from_disk_or_default(&file_path);
        assert_eq!(loaded_struct.value, 42);

        // Load from a non-existent file
        let non_existent_path = dir.path().join("non_existent_file");
        let default_struct: TestStruct =
            TestModule::<usize>::load_from_disk_or_default(&non_existent_path);
        assert_eq!(default_struct.value, 0);
    }

    #[test_log::test]
    fn test_save_on_disk() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file.data");

        let test_struct = TestStruct { value: 42 };
        TestModule::<usize>::save_on_disk(&file_path, &test_struct).unwrap();

        // Load the struct from the file to verify it was saved correctly
        let loaded_struct: TestStruct = TestModule::<usize>::load_from_disk_or_default(&file_path);
        assert_eq!(loaded_struct.value, 42);
    }

    #[tokio::test]
    async fn test_build_module() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut handler = ModulesHandler::new(&shared_bus).await;
        handler
            .build_module::<TestModule<usize>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
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
            _field: 2_usize,
        };

        handler.add_module(module).unwrap();
        assert_eq!(handler.modules.len(), 1);
    }

    #[tokio::test]
    async fn test_start_modules() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut shutdown_receiver = get_receiver::<ShutdownModule>(&shared_bus).await;
        let mut shutdown_completed_receiver = get_receiver::<ShutdownCompleted>(&shared_bus).await;
        let mut handler = ModulesHandler::new(&shared_bus).await;
        handler
            .build_module::<TestModule<usize>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();

        _ = handler.start_modules().await;
        _ = handler.shutdown_next_module().await;

        assert_eq!(
            shutdown_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<usize>>().to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<usize>>().to_string()
        );
    }

    // When modules are strated in the following order A, B, C, they should be closed in the reverse order C, B, A
    #[tokio::test]
    async fn test_start_stop_modules_in_order() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut shutdown_receiver = get_receiver::<ShutdownModule>(&shared_bus).await;
        let mut shutdown_completed_receiver = get_receiver::<ShutdownCompleted>(&shared_bus).await;
        let mut handler = ModulesHandler::new(&shared_bus).await;

        handler
            .build_module::<TestModule<usize>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<String>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        _ = handler.start_modules().await;
        _ = handler.shutdown_modules().await;

        // Shutdown last module first
        assert_eq!(
            shutdown_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<String>>().to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<String>>().to_string()
        );

        // Then first module at last
        assert_eq!(
            shutdown_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<usize>>().to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<usize>>().to_string()
        );
    }

    #[tokio::test]
    async fn test_shutdown_modules_exactly_once() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut cancellation_counter_receiver = get_receiver::<usize>(&shared_bus).await;
        let mut shutdown_completed_receiver = get_receiver::<ShutdownCompleted>(&shared_bus).await;
        let mut handler = ModulesHandler::new(&shared_bus).await;

        handler
            .build_module::<TestModule<usize>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<String>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<bool>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();

        _ = handler.start_modules().await;
        _ = tokio::time::sleep(Duration::from_millis(100)).await;
        _ = handler.shutdown_modules().await;

        // Shutdown last module first
        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<bool>>().to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<String>>().to_string()
        );

        // Then first module at last

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<usize>>().to_string()
        );

        assert_eq!(cancellation_counter_receiver.try_recv().expect("1"), 1);
        assert_eq!(cancellation_counter_receiver.try_recv().expect("1"), 1);
        assert_eq!(cancellation_counter_receiver.try_recv().expect("1"), 1);
    }

    // in case a module fails, it will emit a shutdowncompleted event that will trigger the shutdown loop and shut all other modules
    // the module panic listener will also emit an event because the task ended (and it does not know why)
    // That is why in case of a graceful failure, the shutdown loop receives 2 events for the failed module
    // All other modules are shut in the right order
    #[tokio::test]
    async fn test_shutdown_all_modules_if_one_fails() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut shutdown_completed_receiver = get_receiver::<ShutdownCompleted>(&shared_bus).await;
        let mut handler = ModulesHandler::new(&shared_bus).await;

        handler
            .build_module::<TestModule<usize>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<String>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<u64>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<bool>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();

        _ = handler.start_modules().await;

        // Starting shutdown loop should shut all modules because one failed immediatly

        _ = handler.shutdown_loop().await;

        // u64 module fails first, emits two events, one because it is the first task to end,
        // and the other because it finished to shutdown corretly
        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<u64>>().to_string()
        );
        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<u64>>().to_string()
        );
        // Shutdown last module first
        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<bool>>().to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<String>>().to_string()
        );

        // Then first module at last

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<usize>>().to_string()
        );
    }

    // in case a module panics,
    // the module panic listener will know the task has ended, and will trigger a shutdown completed event
    // the other modules will shut in the right order
    #[tokio::test]
    async fn test_shutdown_all_modules_if_one_module_panics() {
        let shared_bus = SharedMessageBus::new(BusMetrics::global("id".to_string()));
        let mut shutdown_completed_receiver = get_receiver::<ShutdownCompleted>(&shared_bus).await;
        let mut handler = ModulesHandler::new(&shared_bus).await;

        handler
            .build_module::<TestModule<usize>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<String>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<u32>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();
        handler
            .build_module::<TestModule<bool>>(
                TestBusClient::new_from_bus(shared_bus.new_handle()).await,
            )
            .await
            .unwrap();

        _ = handler.start_modules().await;

        // Starting shutdown loop should shut all modules because one failed immediatly

        _ = handler.shutdown_loop().await;

        // u32 module failed with panic, but the event should be emitted

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<u32>>().to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<bool>>().to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<String>>().to_string()
        );

        assert_eq!(
            shutdown_completed_receiver.recv().await.unwrap().module,
            std::any::type_name::<TestModule<usize>>().to_string()
        );
    }
}
