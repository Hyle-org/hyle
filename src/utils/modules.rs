use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
};

use tracing::info;

/// Module trait to define startup dependencies
pub trait Module {
    fn name() -> &'static str;
    fn dependencies() -> Vec<&'static str>;
}

struct ModuleStarter {
    dependencies: Vec<&'static str>,
    starter: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
}

impl ModuleStarter {
    fn start(self, name: &'static str) {
        info!("Starting module {}", name);
        let _ = tokio::task::Builder::new().name(name).spawn(self.starter);
    }
}

#[derive(Default)]
pub struct ModulesHandler {
    modules: HashMap<&'static str, ModuleStarter>,
}

impl ModulesHandler {
    pub fn add_module<M>(&mut self, starter: impl Future<Output = ()> + Send + 'static)
    where
        M: Module,
    {
        self.modules.insert(
            M::name(),
            ModuleStarter {
                dependencies: M::dependencies(),
                starter: Box::pin(starter),
            },
        );
    }

    fn topological_sort(&self) -> Result<Vec<&'static str>, &'static str> {
        let mut sorted = Vec::new();
        let mut visited = HashSet::new();
        let mut temp_marked = HashSet::new();

        fn visit(
            module_name: &'static str,
            modules: &HashMap<&'static str, ModuleStarter>,
            visited: &mut HashSet<&'static str>,
            temp_marked: &mut HashSet<&'static str>,
            sorted: &mut Vec<&'static str>,
        ) -> Result<(), &'static str> {
            if temp_marked.contains(module_name) {
                return Err("Cycle detected in module dependencies");
            }

            if !visited.contains(module_name) {
                temp_marked.insert(module_name);
                if let Some(module_starter) = modules.get(module_name) {
                    for dep in module_starter.dependencies.clone() {
                        visit(dep, modules, visited, temp_marked, sorted)?;
                    }
                }
                temp_marked.remove(module_name);
                visited.insert(module_name);
                sorted.push(module_name);
            }
            Ok(())
        }

        for module_name in self.modules.keys() {
            visit(
                module_name,
                &self.modules,
                &mut visited,
                &mut temp_marked,
                &mut sorted,
            )?;
        }

        Ok(sorted)
    }

    /// Start Modules
    pub fn start_modules(&mut self) -> Result<(), &'static str> {
        let sorted_modules = self.topological_sort()?;

        for module_name in sorted_modules {
            if let Some(module) = self.modules.remove(module_name) {
                module.start(module_name);
            }
        }

        Ok(())
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

        fn dependencies() -> Vec<&'static str> {
            vec![]
        }
    }

    struct ModuleB;
    impl Module for ModuleB {
        fn name() -> &'static str {
            "B"
        }

        fn dependencies() -> Vec<&'static str> {
            vec!["A"]
        }
    }

    struct ModuleC;
    impl Module for ModuleC {
        fn name() -> &'static str {
            "C"
        }

        fn dependencies() -> Vec<&'static str> {
            vec!["A"]
        }
    }

    struct ModuleD;
    impl Module for ModuleD {
        fn name() -> &'static str {
            "D"
        }

        fn dependencies() -> Vec<&'static str> {
            vec!["B", "C"]
        }
    }

    #[tokio::test]
    async fn test_modules_start() {
        // Initialiser les modules et les ajouter à un HashMap

        let mut handler = ModulesHandler::default();
        handler.add_module::<ModuleA>(async { info!("Starting module A") });
        handler.add_module::<ModuleC>(async { info!("Starting module C") });
        handler.add_module::<ModuleB>(async { info!("Starting module B") });
        handler.add_module::<ModuleD>(async { info!("Starting module D") });

        // Démarrer les modules dans l'ordre topologique
        assert!(handler.start_modules().is_ok());
    }

    struct ModuleC2;
    impl Module for ModuleC2 {
        fn name() -> &'static str {
            "C"
        }

        fn dependencies() -> Vec<&'static str> {
            vec!["B", "D"]
        }
    }

    #[tokio::test]
    async fn test_modules_cycles() {
        // Initialiser les modules et les ajouter à un HashMap
        let mut handler = ModulesHandler::default();

        handler.add_module::<ModuleA>(async { info!("Starting module A") });
        handler.add_module::<ModuleC2>(async { info!("Starting module C") });
        handler.add_module::<ModuleB>(async { info!("Starting module B") });
        handler.add_module::<ModuleD>(async { info!("Starting module D") });

        assert!(handler.start_modules().is_err());
    }
}
