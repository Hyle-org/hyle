use std::time::Duration;

use tokio::sync::mpsc;
use tracing::info;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();

    for i in 1..10 {
        let s = sender.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;

                info!("Making a server call");
                // Call server

                let _ = s
                    .send(format!("Hello from {}", i))
                    .context("Sending message");
            }
        });
    }

    while let Some(msg) = receiver.recv().await {
        info!("Received message from {msg}");
    }

    Ok(())
}
