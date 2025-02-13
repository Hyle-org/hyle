use clap::{Parser, Subcommand};
use client_sdk::helpers::risc0::Risc0Prover;
use client_sdk::rest_client::NodeApiHttpClient;
use client_sdk::transaction_builder::ProvableBlobTx;
use client_sdk::transaction_builder::TxExecutor;
use client_sdk::transaction_builder::TxExecutorBuilder;
use hydentity::client::metadata::HYDENTITY_ELF;
use hydentity::client::HydentityPseudoExecutor;
use hydentity::Hydentity;
use sdk::identity_provider::IdentityAction;
use sdk::ContractName;
use sdk::Hashable;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, default_value = "http://localhost:4321")]
    pub host: String,
}

#[derive(Subcommand)]
enum Commands {
    RegisterIdentity {
        identity: String,
        password: String,
    },
    VerifyIdentity {
        identity: String,
        password: String,
        nonce: u32,
    },
}

async fn build_ctx(client: &NodeApiHttpClient) -> TxExecutor<Hydentity> {
    let contract_name = "hydentity".into();
    // Fetch the initial state from the node
    let initial_state: Hydentity = client
        .get_contract(&contract_name)
        .await
        .unwrap()
        .state
        .try_into()
        .unwrap();

    let mut tx_builder = TxExecutorBuilder::default();
    tx_builder.with_contract(
        contract_name,
        initial_state,
        HydentityPseudoExecutor {},
        Risc0Prover::new(HYDENTITY_ELF),
    );

    tx_builder.build()
}

#[tokio::main]
async fn main() {
    // Initialize tracing. In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    let client = client_sdk::rest_client::NodeApiHttpClient::new(cli.host).unwrap();

    let mut ctx = build_ctx(&client).await;

    match cli.command {
        Commands::RegisterIdentity { identity, password } => {
            // ----
            // Build the blob transaction
            // ----

            let mut transaction = ProvableBlobTx::new(identity.clone().into());

            hydentity::client::register_identity(
                &mut transaction,
                ContractName::new("hydentity"),
                password,
            )
            .unwrap();

            let transaction = ctx.process(transaction).unwrap();

            // Send the blob transaction
            let blob_tx_hash = client
                .send_tx_blob(&transaction.to_blob_tx())
                .await
                .unwrap();
            println!("✅ Blob tx sent. Tx hash: {}", blob_tx_hash);

            // ----
            // Prove the state transition
            // ----
            for proof in transaction.iter_prove() {
                let tx = proof.await.unwrap();
                client.send_tx_proof(&tx).await.unwrap();
                println!(
                    "✅ Proof tx sent for {}. Tx hash: {}",
                    tx.contract_name,
                    tx.hash()
                );
            }
        }
        Commands::VerifyIdentity {
            identity,
            password,
            nonce,
        } => {
            // ----
            // Build the blob transaction
            // ----

            let mut transaction = ProvableBlobTx::new(identity.clone().into());

            transaction
                .add_action(
                    "hydentity".into(),
                    IdentityAction::VerifyIdentity {
                        account: identity.clone(),
                        nonce,
                    },
                    Some(password.into_bytes().to_vec()),
                    None,
                    None,
                )
                .unwrap();

            let transaction = ctx.process(transaction).unwrap();

            // Send the blob transaction
            let blob_tx_hash = client
                .send_tx_blob(&transaction.to_blob_tx())
                .await
                .unwrap();
            println!("✅ Blob tx sent. Tx hash: {}", blob_tx_hash);

            // ----
            // Prove the state transition
            // ----
            for proof in transaction.iter_prove() {
                let tx = proof.await.unwrap();
                client.send_tx_proof(&tx).await.unwrap();
                println!(
                    "✅ Proof tx sent for {}. Tx hash: {}",
                    tx.contract_name,
                    tx.hash()
                );
            }
        }
    }
}
