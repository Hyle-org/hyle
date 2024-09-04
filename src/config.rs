use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub addr: String,
}

pub async fn read(path: &str) -> Result<Config> {
    let config_data = fs::read_to_string(path)
        .await
        .context("reading config file")?;

    ron::from_str(&config_data).context("deserializing config file")
}
