use anyhow::Result;
use client_sdk::contract_states;
use client_sdk::helpers::test::TestProver;
use client_sdk::tcp_client::NodeTcpClient;
use client_sdk::transaction_builder::{ProvableBlobTx, TxExecutorBuilder};
use hydentity::Hydentity;
use hyle_contract_sdk::BlobTransaction;
use hyle_contract_sdk::Identity;
use hyle_contract_sdk::{guest, ContractInput, ContractName, HyleOutput};
use hyle_contract_sdk::{Blob, BlobData, ContractAction, RegisterContractAction};
use hyle_contract_sdk::{Digestable, TcpServerNetMessage};
use hyllar::client::transfer;
use hyllar::{Hyllar, HyllarRegisterAction};
use tokio::task::JoinSet;
use tracing::info;

contract_states!(
    #[derive(Debug, Clone)]
    pub struct States {
        pub hydentity: Hydentity,
        pub hyllar_test: Hyllar,
    }
);

pub fn setup_hyllar(_users: u32) -> Result<(HyllarRegisterAction, Hyllar)> {
    let register = HyllarRegisterAction {
        initial_supply: 0,
        faucet_id: "faucet.hyllar_test".into(),
    };
    let hyllar = Hyllar::register(register.clone());
    Ok((register, hyllar))
}

/// Create a new contract "hyllar_test" that already contains entries for each users
pub async fn setup(url: String, users: u32, verifier: String) -> Result<()> {
    let (register, hyllar) = setup_hyllar(users)?;

    let tx = BlobTransaction::new(
        Identity::new("hyle.hyle"),
        vec![RegisterContractAction {
            contract_name: "hyllar_test".into(),
            verifier: verifier.into(),
            program_id: hyle_contracts::HYLLAR_ID.to_vec().into(),
            state_digest: hyllar.as_digest(),
            register_action: register.as_blob_data()?,
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
