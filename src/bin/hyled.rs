use std::{fs::File, io::Read};

use anyhow::Result;
use clap::{command, Parser, Subcommand};
use hyle::{
    model::{
        Blob, BlobData, BlobReference, BlobTransaction, ContractName, ProofTransaction,
        RegisterContractTransaction,
    },
    rest::client::ApiHttpClient,
};
use reqwest::{Client, Url};
use sdk::{BlobIndex, Identity, StateDigest, TxHash};

pub fn load_encoded_receipt_from_file(path: &str) -> Vec<u8> {
    let mut file = File::open(path).expect("Failed to open proof file");
    let mut encoded_receipt = Vec::new();
    file.read_to_end(&mut encoded_receipt)
        .expect("Failed to read file content");
    encoded_receipt
}

async fn send_proof(
    client: &ApiHttpClient,
    blob_tx_hash: TxHash,
    blob_index: BlobIndex,
    contract_name: ContractName,
    proof_file: String,
) -> Result<()> {
    let proof = load_encoded_receipt_from_file(proof_file.as_str());
    let res = client
        .send_tx_proof(&ProofTransaction {
            blobs_references: vec![BlobReference {
                contract_name,
                blob_tx_hash,
                blob_index,
            }],
            proof,
        })
        .await?;
    assert!(res.status().is_success());

    println!("Proof sent successfully");
    println!("Response: {}", res.text().await?);

    Ok(())
}

async fn send_blob(
    client: &ApiHttpClient,
    identity: Identity,
    contract_name: ContractName,
    blob_data: String,
) -> Result<()> {
    let data = BlobData(hex::decode(blob_data).expect("Data decoding failed"));
    let res = client
        .send_tx_blob(&BlobTransaction {
            identity,
            blobs: vec![Blob {
                contract_name,
                data,
            }],
        })
        .await?;

    assert!(res.status().is_success());

    println!("Blob sent successfully");
    println!("Response: {}", res.text().await?);

    Ok(())
}

async fn register_contracts(
    client: &ApiHttpClient,
    owner: String,
    verifier: String,
    program_hex_id: String,
    state_hex_digest: String,
    contract_name: ContractName,
) -> Result<()> {
    let program_id = hex::decode(program_hex_id).expect("Image id decoding failed");
    let state_digest =
        StateDigest(hex::decode(state_hex_digest).expect("State digest decoding failed"));
    let res = client
        .send_tx_register_contract(&RegisterContractTransaction {
            owner,
            verifier,
            program_id,
            state_digest,
            contract_name,
        })
        .await?;

    assert!(res.status().is_success());

    println!("Contract registered");
    println!("Response: {}", res.text().await?);

    Ok(())
}

/// A cli to interact with hyle node
#[derive(Debug, Parser)] // requires `derive` feature
#[command(name = "hyled")]
#[command(about = "A CLI to interact with hyle", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: SendCommands,

    #[arg(long, default_value = "localhost")]
    pub host: String,

    #[arg(long, default_value = "4321")]
    pub port: u32,
}

#[derive(Debug, Subcommand)]
enum SendCommands {
    /// Send blob transaction
    #[command(alias = "b")]
    Blob {
        identity: String,
        contract_name: String,
        data: String,
    },
    /// Send proof transaction
    #[command(alias = "p")]
    Proof {
        tx_hash: String,
        blob_index: u32,
        contract_name: String,
        proof_file: String,
    },
    /// Register contract
    #[command(alias = "c")]
    Contract {
        owner: String,
        verifier: String,
        program_id: String,
        contract_name: String,
        state_digest: String,
    },
    Auto,
}

async fn handle_args(args: Args) -> Result<()> {
    let url = format!("http://{}:{}", args.host, args.port);

    let client = ApiHttpClient {
        url: Url::parse(url.as_str()).unwrap(),
        reqwest_client: Client::new(),
    };

    match args.command {
        SendCommands::Blob {
            identity,
            contract_name,
            data,
        } => send_blob(&client, identity.into(), contract_name.into(), data).await,
        SendCommands::Proof {
            tx_hash,
            blob_index,
            contract_name,
            proof_file,
        } => {
            send_proof(
                &client,
                tx_hash.into(),
                blob_index.into(),
                contract_name.into(),
                proof_file,
            )
            .await
        }

        SendCommands::Contract {
            verifier,
            owner,
            program_id,
            contract_name,
            state_digest,
        } => {
            register_contracts(
                &client,
                owner,
                verifier,
                program_id,
                state_digest,
                contract_name.into(),
            )
            .await
        }
        SendCommands::Auto => {
            let _ = client
                .run_scenario_api_test()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to run scenario test {}", e))?;
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    match handle_args(args).await {
        Ok(_) => println!("Success"),
        Err(e) => eprintln!("Error: {:?}", e),
    }
}
