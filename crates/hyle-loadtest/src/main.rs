use anyhow::Error;
use clap::{Parser, Subcommand};
use hyle_loadtest::{generate, send, setup};
use tracing::Level;

/// A cli to interact with hyle node
#[derive(Debug, Parser)] // requires `derive` feature
#[command(name = "loadtest")]
#[command(about = "A CLI to loadtest hyle", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: SendCommands,

    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    #[arg(long, default_value = "4321")]
    pub port: u32,

    #[arg(long, default_value = "10")]
    pub users: u32,

    #[arg(long, default_value = "test")]
    pub verifier: String,
}

#[derive(Debug, Subcommand)]
enum SendCommands {
    /// Register Contracts
    #[command(alias = "s")]
    Setup,
    /// Generates Blob and Proof transactions for the load test
    #[command(alias = "gt")]
    GenerateTransactions,
    /// Load the transactions and send them
    #[command(alias = "st")]
    SendTransactions,
    /// Run the entire flow
    #[command(alias = "l")]
    LoadTest,
    /// Generate 100k blob transactions and send them to timeout
    #[command(alias = "to")]
    TimeoutScenario,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let args = Args::parse();

    let url = format!("http://{}:{}", args.host, args.port);
    let users = args.users;

    match args.command {
        SendCommands::Setup => setup(url).await?,
        SendCommands::GenerateTransactions => generate(url, users).await?,
        SendCommands::SendTransactions => send(url).await?,
        SendCommands::LoadTest => {
            setup(url.clone()).await?;
            generate(url.clone(), users).await?;
            send(url).await?;
        }
        SendCommands::TimeoutScenario => {
            todo!()
        }
    }

    Ok(())
}
