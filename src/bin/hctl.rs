use anyhow::Result;
use borsh::BorshDeserialize;
use clap::{Parser, Subcommand};
use client_sdk::rest_client::IndexerApiHttpClient;
use hyle::utils::logger::{setup_tracing, TracingMode};
use hyle_contract_sdk::{erc20::ERC20Action, identity_provider::IdentityAction};
use hyle_model::{
    api::{APIBlob, APITransaction},
    BlockHeight, ConsensusProposalHash, RegisterContractAction, StakingAction, StructuredBlobData,
    TxHash,
};
use termion::color;

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, default_value = "http://localhost:4321")]
    pub host: String,
}

#[derive(Subcommand)]
enum Commands {
    Inspect {
        value: String,

        #[arg(short, long, action = clap::ArgAction::SetTrue)]
        verbose: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    setup_tracing(TracingMode::Full, "hctl".into())?;

    let indexer = IndexerApiHttpClient::new(args.host)?;

    match args.command {
        Commands::Inspect { value, verbose } => {
            if let Ok(height) = value.parse() {
                if let Ok(block) = indexer.get_block_by_height(&BlockHeight(height)).await {
                    inspect_block(&indexer, block, verbose).await?;
                } else {
                    println!("Block not found at height {}", height);
                }
            } else if let Ok(tx) = indexer
                .get_transaction_with_hash(&TxHash(value.clone()))
                .await
            {
                inspect_transaction(&indexer, tx).await?;
            } else if let Ok(block) = indexer
                .get_block_by_hash(&ConsensusProposalHash(value.clone()))
                .await
            {
                inspect_block(&indexer, block, verbose).await?;
            } else {
                println!("Hash not found");
            }
        }
    };

    Ok(())
}

async fn inspect_block(
    indexer: &IndexerApiHttpClient,
    block: hyle_model::api::APIBlock,
    verbose: bool,
) -> Result<()> {
    println!("Block {}", block.height);
    println!("Parent Hash: {}", block.parent_hash);
    println!("Parent: {}", block.parent_hash);
    let txs = indexer
        .get_transactions_by_height(&BlockHeight(block.height))
        .await?;
    println!("Tx count: {}", txs.len());
    for tx in txs {
        if verbose {
            println!("- Transaction {}", tx.tx_hash);
            inspect_transaction(indexer, tx).await?;
            println!(
                "------------------------------------------------------------------------------"
            );
        } else {
            println!("- Tx {} ({})", tx.tx_hash, tx.transaction_type);
        }
    }
    Ok(())
}

async fn inspect_transaction(indexer: &IndexerApiHttpClient, tx: APITransaction) -> Result<()> {
    println!("Type: {:?}", tx.transaction_type);
    println!("Status: {:?}", tx.transaction_status);
    if !tx.parent_dp_hash.0.is_empty() {
        println!("Parent DP: {}", tx.parent_dp_hash);
    }
    if let Some(block_hash) = tx.block_hash {
        println!("Block: Hash:  {}", block_hash);
    }
    if let Some(index) = tx.index {
        println!("       Index: {}", index);
    }
    match tx.transaction_type {
        hyle_model::api::TransactionTypeDb::BlobTransaction => {
            let blobs = indexer.get_blobs_by_tx_hash(&tx.tx_hash).await?;
            println!("Blobs count: {}", blobs.len());
            for blob in blobs {
                display_blob(blob);
            }
        }
        hyle_model::api::TransactionTypeDb::ProofTransaction => {}
        _ => {}
    };

    Ok(())
}

fn display_blob(blob: APIBlob) {
    if blob.verified {
        print!("{}", color::Fg(color::Green));
    } else {
        print!("{}", color::Fg(color::Red));
    }

    println!(
        "----- index: {} - contract : {} ----",
        blob.blob_index, blob.contract_name
    );
    match blob.contract_name.as_str() {
        "hyllar" | "hyllar2" => {
            parse_and_print::<StructuredBlobData<ERC20Action>>(&blob.data);
        }
        "hydentity" => {
            parse_and_print::<IdentityAction>(&blob.data);
        }
        "staking" => {
            parse_and_print::<StructuredBlobData<StakingAction>>(&blob.data);
        }
        "hyle" => {
            parse_and_print::<StructuredBlobData<RegisterContractAction>>(&blob.data);
        }
        _ => {
            println!("Unknown format");
        }
    };
    print!("{}", color::Fg(color::Reset));
}

fn parse_and_print<T>(d: &[u8])
where
    T: BorshDeserialize + HctlDisplay,
{
    let parsed = borsh::from_slice::<T>(d);
    match parsed {
        Ok(v) => v.display(),
        Err(e) => println!("Failed to parse {}: {:?}", std::any::type_name::<T>(), e),
    }
}

trait HctlDisplay {
    fn display(&self);
}

impl<T> HctlDisplay for StructuredBlobData<T>
where
    T: HctlDisplay,
{
    fn display(&self) {
        if let Some(caller) = &self.caller {
            println!("caller index: {}", caller);
        }
        if let Some(callees) = &self.callees {
            println!("callees: {:?}", callees);
        }
        self.parameters.display();
    }
}

impl HctlDisplay for IdentityAction {
    fn display(&self) {
        match self {
            IdentityAction::RegisterIdentity { account } => println!("-> Register {account}"),
            IdentityAction::VerifyIdentity { account, nonce } => {
                println!("-> Verify {account} nonce={nonce}")
            }
            IdentityAction::GetIdentityInfo { account } => println!("-> Get {account} info"),
        }
    }
}

impl HctlDisplay for ERC20Action {
    fn display(&self) {
        match self {
            ERC20Action::TotalSupply => println!("-> TotalSupply"),
            ERC20Action::BalanceOf { account } => println!("-> BalanceOf {account}"),
            ERC20Action::Transfer { recipient, amount } => {
                println!("-> Transfer {amount} -> {recipient}")
            }
            ERC20Action::TransferFrom {
                owner,
                recipient,
                amount,
            } => println!("-> TransferFrom {amount} from {owner} -> {recipient}"),
            ERC20Action::Approve { spender, amount } => {
                println!("-> Approve {amount} -> {spender}")
            }
            ERC20Action::Allowance { owner, spender } => {
                println!("-> Allowance {owner} -> {spender}")
            }
        }
    }
}

impl HctlDisplay for StakingAction {
    fn display(&self) {
        match self {
            StakingAction::Stake { amount } => println!("-> Stake {amount}"),
            StakingAction::Delegate { validator } => println!("-> Delegate to {validator}"),
            StakingAction::Distribute { claim } => {
                println!("-> Distribute rewards for blocks: {:?}", claim)
            }
            StakingAction::DepositForFees { holder, amount } => {
                println!("-> Deposit {amount} for fees to {holder}")
            }
        }
    }
}

impl HctlDisplay for RegisterContractAction {
    fn display(&self) {
        println!(
            "-> Register contract {} with verifier {}",
            self.contract_name, self.verifier
        );
    }
}
