use amm::UnorderedTokenPair;
use anyhow::Error;
use clap::{Parser, Subcommand};
use hyle::model::{BlobTransaction, ProofTransaction};
use hyle::rest::client::ApiHttpClient;
use hyle_loadtest::{
    create_transactions, register_contracts, send_transactions, setup_contract_states,
};
use reqwest::{Client, Url};
use std::fs::File;
use std::io::{Read, Write};

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
    #[command(alias = "rc")]
    RegisterContracts,
    /// Generates Blob and Proof transactions for the load test
    #[command(alias = "gt")]
    GenerateTransactions,
    /// Load the transactions and send them
    #[command(alias = "st")]
    SendTransactions,
    /// Run the entire flow
    #[command(alias = "l")]
    LoadTest,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Args::parse();

    let url = format!("http://{}:{}", args.host, args.port);
    let client = ApiHttpClient {
        url: Url::parse(&url).unwrap(),
        reqwest_client: Client::new(),
    };
    let verifier = args.verifier;

    let pair = UnorderedTokenPair::new("token1-loadtest".to_owned(), "token2-loadtest".to_owned());

    let (mut hydentity_state, mut amm_state, mut token1_state, mut token2_state) =
        setup_contract_states(&args.users, &pair);

    match args.command {
        SendCommands::RegisterContracts => {
            register_contracts(
                &client,
                &hydentity_state,
                &amm_state,
                &token1_state,
                &token2_state,
            )
            .await?;
        }
        SendCommands::GenerateTransactions => {
            let txs_to_send = create_transactions(
                verifier.as_str(),
                &pair,
                &mut hydentity_state,
                &mut amm_state,
                &mut token1_state,
                &mut token2_state,
                args.users,
            )
            .await?;

            let encoded: Vec<u8> =
                bincode::encode_to_vec(&txs_to_send, bincode::config::standard())?;
            let mut file =
                File::create(format!("./txs_to_send.{verifier}.{}-users.bin", args.users))?;
            file.write_all(&encoded)?;
        }
        SendCommands::SendTransactions => {
            let mut file =
                File::open(format!("./txs_to_send.{verifier}.{}-users.bin", args.users))?;
            let mut encoded = Vec::new();
            file.read_to_end(&mut encoded)?;

            let txs_to_send: Vec<(BlobTransaction, Vec<ProofTransaction>)> =
                bincode::decode_from_slice(&encoded, bincode::config::standard())
                    .map(|(data, _)| data)?;

            send_transactions(&client, txs_to_send).await?;
        }
        SendCommands::LoadTest => {
            register_contracts(
                &client,
                &hydentity_state,
                &amm_state,
                &token1_state,
                &token2_state,
            )
            .await?;

            let txs_to_send = create_transactions(
                verifier.as_str(),
                &pair,
                &mut hydentity_state,
                &mut amm_state,
                &mut token1_state,
                &mut token2_state,
                args.users,
            )
            .await?;

            let encoded: Vec<u8> =
                bincode::encode_to_vec(&txs_to_send, bincode::config::standard())?;
            let mut file =
                File::create(format!("./txs_to_send.{verifier}.{}-users.bin", args.users))?;
            file.write_all(&encoded)?;

            send_transactions(&client, txs_to_send).await?;
        }
    };

    Ok(())
}
