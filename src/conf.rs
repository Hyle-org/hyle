use anyhow::{Context, Result};
use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Conf {
    peers: Vec<String>,
    pub storage: Storage,
    rest: String,
}

impl Conf {
    pub fn addr(&self, id: usize) -> Option<&str> {
        return self.peers.get(id).map(String::as_str);
    }

    pub fn rest_addr(&self) -> &str {
        return self.rest.as_str();
    }

    pub fn new() -> Result<Self, ConfigError> {
        let s = Config::builder()
            // Start off by merging in the "default" configuration file
            .add_source(File::with_name("config.ron"))
            .add_source(Environment::with_prefix("hyle"))
            // You may also programmatically change settings
            // .set_override("database.url", "postgres://")?
            .build()?;

        // You can deserialize (and thus freeze) the entire configuration as
        s.try_deserialize()
    }
}
