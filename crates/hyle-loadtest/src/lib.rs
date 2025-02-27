use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use client_sdk::contract_states;
use client_sdk::helpers::risc0::Risc0Prover;
use client_sdk::helpers::test::TestProver;
use client_sdk::rest_client::{IndexerApiHttpClient, NodeApiHttpClient};
use client_sdk::tcp_client::NodeTcpClient;
use client_sdk::transaction_builder::{
    ProvableBlobTx, StateUpdater, TxExecutor, TxExecutorBuilder,
};
use hydentity::client::{register_identity, verify_identity};
use hydentity::Hydentity;
use hyle_contract_sdk::erc20::ERC20;
use hyle_contract_sdk::Identity;
use hyle_contract_sdk::{guest, ContractInput, ContractName, HyleOutput};
use hyle_contract_sdk::{Blob, BlobData, ContractAction, RegisterContractAction};
use hyle_contract_sdk::{BlobTransaction, TxHash};
use hyle_contract_sdk::{Digestable, TcpServerNetMessage};
use hyle_contracts::{HYDENTITY_ELF, HYLLAR_ELF};
use hyllar::client::transfer;
use hyllar::Hyllar;
use rand::Rng;
use tokio::task::JoinSet;
use tracing::info;

contract_states!(
    #[derive(Debug, Clone)]
    pub struct States {
        pub hydentity: Hydentity,
        pub hyllar_test: Hyllar,
    }
);

contract_states!(
    #[derive(Debug, Clone)]
    pub struct CanonicalStates {
        pub hydentity: Hydentity,
        pub hyllar: Hyllar,
    }
);

pub fn setup_hyllar(users: u32) -> Result<Hyllar> {
    let mut hyllar_token = Hyllar::new(0, "faucet.hyllar_test".into());

    // Create an entry for each users
    for n in 0..users {
        let ident = &format!("{n}.hyllar_test");
        hyllar_token
            .transfer(ident, ident, 0)
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    Ok(hyllar_token)
}

/// Create a new contract "hyllar_test" that already contains entries for each users
pub async fn setup(url: String, users: u32, verifier: String) -> Result<()> {
    let hyllar = setup_hyllar(users)?;

    let tx = BlobTransaction::new(
        Identity::new("hyle.hyle"),
        vec![RegisterContractAction {
            contract_name: "hyllar_test".into(),
            verifier: verifier.into(),
            program_id: hyle_contracts::HYLLAR_ID.to_vec().into(),
            state_digest: hyllar.as_digest(),
        }
        .as_blob("hyle".into(), None, None)],
    );

    let mut client = NodeTcpClient::new(url).await.unwrap();
    client.send_transaction(tx.into()).await.unwrap();

    Ok(())
}

pub async fn generate(users: u32, states: States) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
    let blob_txs = generate_blobs_txs(users).await?;
    let proof_txs = generate_proof_txs(users, states).await?;

    Ok((blob_txs, proof_txs))
}

pub async fn generate_blobs_txs(users: u32) -> Result<Vec<Vec<u8>>> {
    let mut blob_txs = vec![];
    let mut tasks = JoinSet::new();
    let number_of_tasks = 100;
    let chunk_size: usize = users.div_ceil(number_of_tasks).try_into().unwrap();

    let user_chunks: Vec<_> = (0..users).collect();
    let user_chunks = user_chunks
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>();

    for chunk in user_chunks {
        tasks.spawn(async move {
            let mut local_blob_txs = vec![];

            for n in &chunk {
                info!(
                    "Building blob transaction for user: {n}/{:?}",
                    chunk.last().unwrap()
                );
                let ident = Identity(format!("{n}.hyllar_test").to_string());

                let mut transaction = ProvableBlobTx::new(ident.clone());
                transfer(
                    &mut transaction,
                    "hyllar_test".into(),
                    ident.clone().to_string(),
                    0,
                )?;

                let identity = transaction.identity;
                let blobs = transaction.blobs;

                let msg: TcpServerNetMessage = BlobTransaction::new(identity, blobs).into();
                local_blob_txs.push(msg.to_binary()?);
            }

            Ok::<_, anyhow::Error>(local_blob_txs)
        });
    }

    while let Some(result) = tasks.join_next().await {
        blob_txs.extend(result??);
    }

    info!("Saving blob transactions");
    std::fs::write(
        format!("blob_txs.{users}.bin"),
        borsh::to_vec(&blob_txs).expect("failed to encode blob_txs"),
    )?;
    Ok(blob_txs)
}

pub async fn generate_proof_txs(users: u32, state: States) -> Result<Vec<Vec<u8>>> {
    let mut proof_txs = vec![];
    let mut tasks = JoinSet::new();
    let number_of_tasks = 100;
    let chunk_size: usize = users.div_ceil(number_of_tasks).try_into().unwrap();

    let user_chunks: Vec<_> = (0..users).collect();
    let user_chunks = user_chunks
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>();
    for chunk in user_chunks {
        let mut ctx = TxExecutorBuilder::new(state.clone())
            .with_prover("hyllar_test".into(), TestProver {})
            .build();
        tasks.spawn(async move {
            let mut local_proof_txs = vec![];

            for n in &chunk {
                info!(
                    "Building proof transaction for user: {n}/{:?}",
                    chunk.last().unwrap()
                );
                let ident = Identity(format!("{n}.hyllar_test").to_string());

                let mut transaction = ProvableBlobTx::new(ident.clone());
                transfer(
                    &mut transaction,
                    "hyllar_test".into(),
                    ident.clone().to_string(),
                    0,
                )?;

                let provable_tx = ctx.process(transaction)?;

                for proof in provable_tx.iter_prove() {
                    let tx = proof.await.unwrap();
                    let msg: TcpServerNetMessage = tx.into();
                    local_proof_txs.push(msg.to_binary()?);
                }
            }

            Ok::<_, anyhow::Error>(local_proof_txs)
        });
    }

    while let Some(result) = tasks.join_next().await {
        proof_txs.extend(result??);
    }

    // serialize to bincode and write to file
    info!("Saving proof transactions");
    std::fs::write(
        format!("proof_txs.{users}.bin"),
        borsh::to_vec(&proof_txs).expect("failed to encode proof_txs"),
    )?;

    Ok(proof_txs)
}

pub async fn send(url: String, blob_txs: Vec<Vec<u8>>, proof_txs: Vec<Vec<u8>>) -> Result<()> {
    send_blob_txs(url.clone(), blob_txs).await?;
    send_proof_txs(url, proof_txs).await?;
    Ok(())
}

pub fn load_blob_txs(users: u32) -> Result<Vec<Vec<u8>>> {
    info!("Loading blob transactions");
    let blob_txs: Vec<Vec<u8>> =
        borsh::from_slice(&std::fs::read(format!("blob_txs.{users}.bin"))?)
            .expect("failed to decode blob_txs.bin");

    Ok(blob_txs)
}

pub fn load_proof_txs(users: u32) -> Result<Vec<Vec<u8>>> {
    info!("Loading proof transactions");
    let proof_txs: Vec<Vec<u8>> =
        borsh::from_slice(&std::fs::read(format!("proof_txs.{users}.bin"))?)
            .expect("failed to decode proof_txs.bin");

    Ok(proof_txs)
}

pub async fn send_blob_txs(url: String, blob_txs: Vec<Vec<u8>>) -> Result<()> {
    info!("Sending blob transactions");

    // Spin out a few tasks to send the transactions in parallel
    let mut tasks = tokio::task::JoinSet::new();
    let number_of_tasks = 20;
    let chunk_size = blob_txs.len().div_ceil(number_of_tasks);
    for chunk in blob_txs.chunks(chunk_size) {
        let chunk = chunk.to_vec();
        let url = url.clone();
        tasks.spawn(async move {
            let mut client = NodeTcpClient::new(url).await.unwrap();
            for encoded_blob_tx in chunk.iter() {
                client
                    .send_encoded_message_no_response(encoded_blob_tx.to_vec())
                    .await
                    .unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            info!("Blob transactions sent: {:?}", chunk.len());
        });
    }
    tasks.join_all().await;

    info!("Transactions sent: {:?} total", blob_txs.len());

    Ok(())
}

pub async fn send_proof_txs(url: String, proof_txs: Vec<Vec<u8>>) -> Result<()> {
    info!("Sending proof transactions");

    // Spin out a few tasks to send the transactions in parallel
    let mut tasks = tokio::task::JoinSet::new();
    let number_of_tasks = 20;
    let chunk_size = proof_txs.len().div_ceil(number_of_tasks);
    for chunk in proof_txs.chunks(chunk_size) {
        let chunk = chunk.to_vec();
        let url = url.clone();
        tasks.spawn(async move {
            let mut client = NodeTcpClient::new(url).await.unwrap();
            for encoded_proof_tx in chunk.iter() {
                client
                    .send_encoded_message_no_response(encoded_proof_tx.to_vec())
                    .await
                    .unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            info!("Transactions sent: {:?}", chunk.len());
        });
    }
    tasks.join_all().await;

    info!("Proof transactions sent: {:?} total", proof_txs.len());

    Ok(())
}

pub fn get_current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

pub async fn send_transaction<S: StateUpdater>(
    client: &NodeApiHttpClient,
    transaction: ProvableBlobTx,
    ctx: &mut TxExecutor<S>,
) -> TxHash {
    let identity = transaction.identity.clone();
    let blobs = transaction.blobs.clone();
    let tx_hash = client
        .send_tx_blob(&BlobTransaction::new(identity, blobs))
        .await
        .unwrap();

    let provable_tx = ctx.process(transaction).unwrap();
    for proof in provable_tx.iter_prove() {
        let tx = proof.await.unwrap();
        client.send_tx_proof(&tx).await.unwrap();
    }
    tx_hash
}

pub async fn long_running_test(url: String) -> Result<()> {
    if true {
        std::env::set_var("RISC0_PROVER", "bonsai");
        std::env::set_var("BONSAI_API_URL", "https://api.bonsai.xyz/");
        std::env::set_var("BONSAI_API_KEY", "nec4UpZfL45P2LRoRP4dw3JTZ2yp82ja9bIjcNWF");
    }

    std::env::set_var("RISC0_DEV_MODE", "true");
    let client = NodeApiHttpClient::new("http://rest-api.devnet.hyle.eu".to_string())?;
    let indexer = IndexerApiHttpClient::new("http://rest-api.devnet.hyle.eu".to_string())?;
    // let client = NodeApiHttpClient::new("http://0.0.0.0:4321".to_string())?;
    // let indexer = IndexerApiHttpClient::new("http://0.0.0.0:4321".to_string())?;

    let mut users: Vec<u64> = vec![];

    let hyllar: Hyllar = indexer
        .fetch_current_state(&ContractName::new("hyllar"))
        .await?;
    let hydentity: Hydentity = indexer
        .fetch_current_state(&ContractName::new("hydentity"))
        .await?;

    let mut tx_ctx = TxExecutorBuilder::new(CanonicalStates { hydentity, hyllar })
        // Replace prover binaries for non-reproducible mode.
        .with_prover("hydentity".into(), Risc0Prover::new(HYDENTITY_ELF))
        .with_prover("hyllar".into(), Risc0Prover::new(HYLLAR_ELF))
        .build();

    loop {
        let now = get_current_timestamp_ms();

        // Create a new user
        if now % 5 == 0 || users.len() < 2 {
            let ident = Identity(format!("{}.hydentity", now));
            users.push(now);

            tracing::info!("Creating identity with 100 tokens: {}", ident);

            // tokio::time::sleep(Duration::from_millis(2000)).await;
            // Register new identity
            let mut transaction = ProvableBlobTx::new(ident.clone());

            _ = register_identity(&mut transaction, "hydentity".into(), "password".to_owned());

            let tx_hash = send_transaction(&client, transaction, &mut tx_ctx).await;
            tracing::info!("Register TX Hash: {:?}", tx_hash);

            // Feed with some token
            tracing::info!("Feeding identity {} with tokens", ident);

            let mut transaction = ProvableBlobTx::new("faucet.hydentity".into());

            verify_identity(
                &mut transaction,
                "hydentity".into(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )?;

            transfer(&mut transaction, "hyllar".into(), ident.0.clone(), 100)?;

            let tx_hash = send_transaction(&client, transaction, &mut tx_ctx).await;
            tracing::info!("Transfer TX Hash: {:?}", tx_hash);

            continue;
        }

        // pick 2 random guys and send some tokens from 1 to another
        info!("Running a transfer between 2 buddies",);

        let mut rng = rand::rng();

        let (guy_1_idx, guy_2_idx): (u32, u32) = (rng.random(), rng.random());

        let users_nb = users.len() as u32;

        let (_, &guy_1_id) = users
            .iter()
            .enumerate()
            .find(|(i, _)| *i as u32 == guy_1_idx % users_nb)
            .unwrap();

        let (_, &guy_2_id) = users
            .iter()
            .enumerate()
            .find(|(i, _)| *i as u32 == (guy_2_idx % users_nb))
            .unwrap();

        if guy_1_id == guy_2_id {
            continue;
        }

        let guy_1_id = format!("{}.hydentity", guy_1_id);
        let guy_2_id = format!("{}.hydentity", guy_2_id);

        // dbg!(&users);

        // dbg!(&tx_ctx.hydentity);
        // dbg!(&tx_ctx.hyllar);

        info!("Getting balances for {} and {}", guy_1_id, guy_2_id);

        let Ok(guy_1_balance) = ERC20::balance_of(&tx_ctx.hyllar, &guy_1_id.to_string()) else {
            tracing::warn!("Balance of {} not found", guy_1_id);
            dbg!(&tx_ctx.hyllar);
            tokio::time::sleep(Duration::from_millis(2000)).await;
            continue;
        };
        let Ok(guy_2_balance) = ERC20::balance_of(&tx_ctx.hyllar, &guy_2_id.to_string()) else {
            tracing::warn!("Balance of {} not found", guy_2_id);
            dbg!(&tx_ctx.hyllar);
            tokio::time::sleep(Duration::from_millis(2000)).await;
            continue;
        };

        if guy_1_balance < 2 {
            continue;
        }

        let amount = rng.random_range(1..guy_1_balance);

        info!(
            "Transfering amount {} from {} to {}",
            amount, guy_1_id, guy_2_id
        );

        let mut transaction = ProvableBlobTx::new(Identity(guy_1_id.clone()));
        verify_identity(
            &mut transaction,
            "hydentity".into(),
            &tx_ctx.hydentity,
            "password".to_string(),
        )?;
        transfer(
            &mut transaction,
            "hyllar".into(),
            guy_2_id.clone(),
            amount as u128,
        )?;

        info!(
            "New balances:\n - {}: {}\n - {}: {}",
            guy_1_id,
            guy_1_balance - amount,
            guy_2_id,
            guy_2_balance + amount
        );

        let tx_hash = send_transaction(&client, transaction, &mut tx_ctx).await;
        tracing::info!("Transfer TX Hash: {:?}", tx_hash);
    }
}

pub async fn send_massive_blob(users: u32, url: String) -> Result<()> {
    let ident = Identity::new("test.hydentity");

    let mut data = vec![];

    for i in 0..6000000 {
        data.push(i as u8);
    }

    let mut txs = vec![];

    for i in 0..users {
        let mut user_data = data.clone();
        user_data.extend_from_slice(&i.to_be_bytes());
        let tx = BlobTransaction::new(
            ident.clone(),
            vec![Blob {
                contract_name: "hydentity".into(),
                data: BlobData(user_data),
            }],
        );
        let msg: TcpServerNetMessage = tx.into();
        let encoded_blob_tx = msg.to_binary()?;
        txs.push(encoded_blob_tx);
    }

    let mut client = NodeTcpClient::new(url).await.unwrap();
    for encoded_blob_tx in txs.iter() {
        client
            .send_encoded_message_no_response(encoded_blob_tx.to_vec())
            .await
            .unwrap();
    }

    Ok(())
}
