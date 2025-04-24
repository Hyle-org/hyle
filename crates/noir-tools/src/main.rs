use std::io::Read;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

fn main() -> std::io::Result<()> {
    // setup tracing
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()
        .unwrap();
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let Some(file) = args.get(1) else {
        eprintln!("No arguments provided.");
        return Ok(());
    };

    let mut file = std::fs::File::open(file)?;
    let mut output = Vec::new();
    file.read_to_end(&mut output)?;

    let ho = hyle_verifiers::noir_utils::parse_noir_output(&output).map_err(|e| {
        eprintln!("Error parsing output: {}", e);
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    })?;

    println!("Parsed output: {:?}", ho);

    Ok(())
}
