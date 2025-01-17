use anyhow::{bail, Result};
use client_sdk::tcp_client::NodeTcpClient;
use client_sdk::transaction_builder::{StateUpdater, TransactionBuilder};
use hydentity::Hydentity;
use hyle_contract_sdk::erc20::ERC20;
use hyle_contract_sdk::{Blob, BlobData};
use hyle_contract_sdk::{BlobTransaction, ProofTransaction, RegisterContractTransaction};
use hyle_contract_sdk::{ContractName, Identity};
use hyle_contract_sdk::{Digestable, TcpServerNetMessage};
use hyllar::{HyllarToken, HyllarTokenContract};
use tokio::task::JoinSet;
use tracing::info;

#[derive(Debug, Clone)]
pub struct States {
    pub hyllar: HyllarToken,
    pub hydentity: Hydentity,
}

impl StateUpdater for States {
    fn update(
        &mut self,
        contract_name: &hyle_contract_sdk::ContractName,
        new_state: hyle_contract_sdk::StateDigest,
    ) -> Result<()> {
        match contract_name.0.as_str() {
            "hyllar-test" => self.hyllar = new_state.try_into()?,
            "hydentity" => self.hydentity = new_state.try_into()?,
            _ => bail!("Unknown contract name: {contract_name}"),
        }
        Ok(())
    }

    fn get(&self, contract_name: &ContractName) -> Result<hyle_contract_sdk::StateDigest> {
        Ok(match contract_name.0.as_str() {
            "hyllar-test" => self.hyllar.as_digest(),
            "hydentity" => self.hydentity.as_digest(),
            _ => bail!("Unknown contract name: {contract_name}"),
        })
    }
}

pub fn setup_hyllar(users: u32) -> Result<HyllarTokenContract> {
    let hyllar_token = HyllarToken::new(0, "faucet.hyllar-test".into());
    let mut hyllar_contract =
        HyllarTokenContract::init(hyllar_token.clone(), "faucet.hyllar-test".into());

    // Create an entry for each users
    for n in 0..users {
        let ident = &format!("{n}.hyllar-test");
        hyllar_contract
            .transfer(ident, 0)
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    Ok(hyllar_contract)
}

/// Create a new contract "hyllar-test" that already contains entries for each users
pub async fn setup(url: String, users: u32, verifier: String) -> Result<()> {
    let hyllar_contract = setup_hyllar(users)?;

    let tx = RegisterContractTransaction {
        contract_name: "hyllar-test".into(),
        owner: "hyle".into(),
        verifier: verifier.into(),
        program_id: hyle_contracts::HYLLAR_ID.to_vec().into(),
        state_digest: hyllar_contract.state().as_digest(),
    };

    let mut client = NodeTcpClient::new(url).await.unwrap();
    client.send_transaction(tx.into()).await.unwrap();

    Ok(())
}

pub async fn generate(users: u32, states: States) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
    let blob_txs = match load_blob_txs(users) {
        Ok(txs) => txs,
        Err(_) => {
            info!("Couldn't find blob transactions, generating new ones");
            generate_blobs_txs(users, states.clone()).await?
        }
    };

    let proof_txs = match load_proof_txs(users) {
        Ok(txs) => txs,
        Err(_) => {
            info!("Couldn't find proof transactions, generating new ones");
            generate_proof_txs(users, states).await?
        }
    };

    Ok((blob_txs, proof_txs))
}

pub async fn generate_blobs_txs(users: u32, states: States) -> Result<Vec<Vec<u8>>> {
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
        let states = states.clone();

        tasks.spawn(async move {
            let mut local_blob_txs = vec![];

            for n in &chunk {
                info!(
                    "Building blob transaction for user: {n}/{:?}",
                    chunk.last().unwrap()
                );
                let ident = Identity(format!("{n}.hyllar-test").to_string());
                let mut transaction = TransactionBuilder::new(ident.clone());
                states
                    .hyllar
                    .builder("hyllar-test".into(), &mut transaction)
                    .transfer(ident.clone().to_string(), 0)?;

                // Extract the identity and blobs from the transaction without building it to prevent loading r0vm
                let identity = transaction.identity.clone();
                let blobs = transaction.blobs.clone();

                let msg: TcpServerNetMessage = BlobTransaction { identity, blobs }.into();
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
        bincode::encode_to_vec(blob_txs.clone(), bincode::config::standard())
            .expect("failed to encode blob_txs"),
    )?;
    Ok(blob_txs)
}

pub async fn generate_proof_txs(users: u32, states: States) -> Result<Vec<Vec<u8>>> {
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
        let mut states = states.clone();

        tasks.spawn(async move {
            let mut local_proof_txs = vec![];

            for n in &chunk {
                info!(
                    "Building proof transaction for user: {n}/{:?}",
                    chunk.last().unwrap()
                );
                let ident = Identity(format!("{n}.hyllar-test").to_string());
                let mut transaction = TransactionBuilder::new(ident.clone());
                states
                    .hyllar
                    .builder("hyllar-test".into(), &mut transaction)
                    .transfer_test(ident.clone().to_string(), 0)?;

                transaction.build(&mut states)?;

                for (proof, contract_name) in transaction.iter_prove() {
                    let msg: TcpServerNetMessage = ProofTransaction {
                        contract_name,
                        proof: proof.await.unwrap(),
                    }
                    .into();
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
        bincode::encode_to_vec(proof_txs.clone(), bincode::config::standard())
            .expect("failed to encode proof_txs"),
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
    let (blob_txs, _): (Vec<Vec<u8>>, _) = bincode::decode_from_slice(
        &std::fs::read(format!("blob_txs.{users}.bin"))?,
        bincode::config::standard(),
    )
    .expect("failed to decode blob_txs.bin");

    Ok(blob_txs)
}

pub fn load_proof_txs(users: u32) -> Result<Vec<Vec<u8>>> {
    info!("Loading proof transactions");
    let (proof_txs, _): (Vec<Vec<u8>>, _) = bincode::decode_from_slice(
        &std::fs::read(format!("proof_txs.{users}.bin"))?,
        bincode::config::standard(),
    )
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

pub async fn send_massive_blob(url: String) -> Result<()> {
    let ident = Identity::new("test.hydentity");

    let mut data = vec![];

    for i in 0..6000000 {
        data.push(i as u8);
    }

    let tx = BlobTransaction {
        identity: ident.clone(),
        blobs: vec![Blob {
            contract_name: "hydentity".into(),
            data: BlobData(data),
        }],
    };
    let msg: TcpServerNetMessage = tx.into();
    let encoded_blob_tx = msg.to_binary()?;

    let mut client = NodeTcpClient::new(url).await.unwrap();
    for _ in 0..100 {
        client
            .send_encoded_message_no_response(encoded_blob_tx.to_vec())
            .await
            .unwrap();
    }

    Ok(())
}
