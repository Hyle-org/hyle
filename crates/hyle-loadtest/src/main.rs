use anyhow::Error;
use clap::{Parser, Subcommand};
use hydentity::Hydentity;
use hyle_loadtest::{
    generate, generate_blobs_txs, generate_proof_txs, send, send_blob_txs, send_proof_txs, setup,
    setup_hyllar, States,
};
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
    /// Generates Blob transactions for the load test
    #[command(alias = "gbt")]
    GenerateBlobTransactions,
    /// Generates Proof transactions for the load test
    #[command(alias = "gpt")]
    GenerateProofTransactions,
    /// Generates Blob and Proof transactions for the load test
    #[command(alias = "gt")]
    GenerateTransactions,
    /// Load the Blob transactions and send them
    #[command(alias = "sbt")]
    SendBlobTransactions,
    /// Load the Proof transactions and send them
    #[command(alias = "spt")]
    SendProofTransactions,
    /// Load the transactions and send them
    #[command(alias = "st")]
    SendTransactions,
    /// Run the entire flow
    #[command(alias = "l")]
    LoadTest,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let args = Args::parse();

    let url = format!("http://{}:{}", args.host, args.port);
    let users = args.users;
    let verifier = args.verifier;

    let states = States {
        hyllar: setup_hyllar(users)?.state(),
        hydentity: Hydentity::default(),
    };

    match args.command {
        SendCommands::Setup => setup(url, users, verifier).await?,
        SendCommands::GenerateBlobTransactions => {
            generate_blobs_txs(users, states).await?;
        }
        SendCommands::GenerateProofTransactions => {
            generate_proof_txs(users, states).await?;
        }
        SendCommands::GenerateTransactions => generate(users, states).await?,
        SendCommands::SendBlobTransactions => send_blob_txs(url).await?,
        SendCommands::SendProofTransactions => send_proof_txs(url).await?,
        SendCommands::SendTransactions => send(url).await?,
        SendCommands::LoadTest => {
            setup(url.clone(), users, verifier.clone()).await?;
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            generate(users, states).await?;
            send(url).await?;
        }
    }

    Ok(())
}
