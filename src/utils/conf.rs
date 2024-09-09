use anyhow::Result;
use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Storage {
    pub interval: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Conf {
    port: u16,
    host: String,
    pub peers: Vec<String>,
    pub storage: Storage,
    rest: String,
}

impl Conf {
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn rest_addr(&self) -> &str {
        return self.rest.as_str();
    }

    pub fn new(config_file: String) -> Result<Self, ConfigError> {
        let s = Config::builder()
            // Start off by merging in the "default" configuration file
            .add_source(File::with_name(config_file.as_str()))
            .add_source(Environment::with_prefix("hyle"))
            // You may also programmatically change settings
            // .set_override("database.url", "postgres://")?
            .build()?;

        // You can deserialize (and thus freeze) the entire configuration as
        s.try_deserialize()
    }
}
