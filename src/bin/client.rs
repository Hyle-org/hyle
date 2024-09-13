use anyhow::{Context, Result};
use clap::{command, Args, Parser, Subcommand};
use config::{Config, ConfigError, Environment, File};
use hyle::{
    model::{
        BlobTransaction, ProofTransaction, RegisterContractTransaction, Transaction,
        TransactionData,
    },
    p2p::{
        network::{MempoolNetMessage, NetMessage},
        stream::send_net_message,
    },
    utils::conf::{self, SharedConf},
};
use serde::de::Deserialize;
use tokio::net::TcpStream;
use tracing::info;

fn load<'de, T: Deserialize<'de>>(file: String) -> Result<T, ConfigError> {
    let s = Config::builder()
        .add_source(File::with_name(file.as_str()))
        .add_source(Environment::with_prefix("hyle"))
        .build()?;

    s.try_deserialize::<T>()
}

fn wrap_net_message(tx: Transaction) -> NetMessage {
    NetMessage::MempoolMessage(MempoolNetMessage::NewTx(tx))
}

fn wrap_tx(tx: TransactionData) -> Transaction {
    Transaction {
        version: 1,
        transaction_data: tx,
        inner: "".to_string(),
    }
}

fn load_blob(file: String) -> Result<Transaction> {
    let blob = load::<BlobTransaction>(file).context("loading blob tx from file")?;
    Ok(wrap_tx(TransactionData::Blob(blob)))
}
fn load_proof(file: String) -> Result<Transaction> {
    let blob = load::<ProofTransaction>(file).context("loading blob tx from file")?;
    Ok(wrap_tx(TransactionData::Proof(blob)))
}
fn load_contract(file: String) -> Result<Transaction> {
    let blob = load::<RegisterContractTransaction>(file).context("loading blob tx from file")?;
    Ok(wrap_tx(TransactionData::RegisterContract(blob)))
}

fn handle_send(send: SendArgs) -> Result<NetMessage> {
    let tx = match send.command {
        SendCommands::Blob { file } => load_blob(file)?,
        SendCommands::B { file } => load_blob(file)?,
        SendCommands::Proof { file } => load_proof(file)?,
        SendCommands::P { file } => load_proof(file)?,
        SendCommands::Contract { file } => load_contract(file)?,
        SendCommands::C { file } => load_contract(file)?,
    };
    info!("Sending tx {:?}", tx);
    Ok(wrap_net_message(tx))
}

fn handle_cli(cli: Cli) -> Result<NetMessage> {
    match cli.command {
        Commands::Send(s) => handle_send(s),
        Commands::S(s) => handle_send(s),
    }
}

async fn client(config: SharedConf, cli: Cli) -> Result<()> {
    let res = handle_cli(cli)?;

    let mut socket = TcpStream::connect(config.addr())
        .await
        .context("connecting to server")?;

    send_net_message(&mut socket, res).await?;

    info!("Done");
    Ok(())
}

/// A cli to interact with hyle node
#[derive(Debug, Parser)] // requires `derive` feature
#[command(name = "hyle")]
#[command(about = "A CLI to interact with hyle node", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, default_value = "master.ron")]
    config_file: String,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Send transactions to node
    #[command(arg_required_else_help = true)]
    Send(SendArgs),
    /// alias to Send
    S(SendArgs),
}

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = false)]
struct SendArgs {
    #[command(subcommand)]
    command: SendCommands,
}

#[derive(Debug, Subcommand)]
enum SendCommands {
    /// Send blob transaction
    Blob { file: String },
    /// alias to blob
    B { file: String },
    /// Send proof transaction
    Proof { file: String },
    /// alias to proof
    P { file: String },
    /// Register contract
    Contract { file: String },
    /// alias to contract
    C { file: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let config = conf::Conf::new_shared(cli.config_file.clone())?;

    client(config, cli).await
}
