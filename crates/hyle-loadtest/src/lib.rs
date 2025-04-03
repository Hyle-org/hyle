use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use client_sdk::helpers::risc0::Risc0Prover;
use client_sdk::helpers::test::TestProver;
use client_sdk::rest_client::NodeApiHttpClient;
use client_sdk::tcp_client::{codec_tcp_server, TcpServerMessage};
use client_sdk::transaction_builder::{
    ProvableBlobTx, StateUpdater, TxExecutor, TxExecutorBuilder, TxExecutorHandler,
};
use client_sdk::{contract_states, transaction_builder};
use hydentity::client::tx_executor_handler::{register_identity, verify_identity};
use hydentity::Hydentity;
use hyle_contract_sdk::Identity;
use hyle_contract_sdk::TxHash;
use hyle_contract_sdk::{Blob, BlobData, ContractAction, RegisterContractAction};
use hyle_contract_sdk::{BlobTransaction, Transaction};
use hyle_contract_sdk::{Calldata, ContractName, HyleOutput, ZkContract};
use hyle_contracts::{HYDENTITY_ELF, HYLLAR_ELF};
use hyllar::client::tx_executor_handler::transfer;
use hyllar::erc20::ERC20;
use hyllar::{Hyllar, FAUCET_ID};
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

pub struct CanonicalStates {
    pub hydentity: Hydentity,
    pub hydentity_name: ContractName,
    pub hyllar: Hyllar,
    pub hyllar_name: ContractName,
}

impl transaction_builder::StateUpdater for CanonicalStates {
    fn setup(&self, ctx: &mut TxExecutorBuilder<Self>) {
        self.hydentity
            .setup_builder(self.hydentity_name.clone(), ctx);
        self.hyllar.setup_builder(self.hyllar_name.clone(), ctx);
    }

    fn update(
        &mut self,
        contract_name: &ContractName,
        new_state: &mut dyn std::any::Any,
    ) -> anyhow::Result<()> {
        if contract_name == &self.hydentity_name {
            let Some(st) = new_state.downcast_mut::<Hydentity>() else {
                anyhow::bail!(
                    "Incorrect state data passed for contract '{}'",
                    contract_name
                );
            };
            std::mem::swap(&mut self.hydentity, st);
        } else if contract_name == &self.hyllar_name {
            let Some(st) = new_state.downcast_mut::<Hyllar>() else {
                anyhow::bail!(
                    "Incorrect state data passed for contract '{}'",
                    contract_name
                );
            };
            std::mem::swap(&mut self.hyllar, st);
        } else {
            anyhow::bail!("Unknown contract name: {contract_name}");
        }
        Ok(())
    }

    fn get(&self, contract_name: &ContractName) -> anyhow::Result<Box<dyn std::any::Any>> {
        if contract_name == &self.hydentity_name {
            Ok(Box::new(self.hydentity.clone()))
        } else if contract_name == &self.hyllar_name {
            Ok(Box::new(self.hyllar.clone()))
        } else {
            anyhow::bail!("Unknown contract name: {contract_name}");
        }
    }

    fn execute(
        &mut self,
        contract_name: &ContractName,
        calldata: &Calldata,
    ) -> anyhow::Result<HyleOutput> {
        if contract_name == &self.hydentity_name {
            self.hydentity
                .handle(calldata)
                .map_err(|e| anyhow::anyhow!(e))
        } else if contract_name == &self.hyllar_name {
            self.hyllar.handle(calldata).map_err(|e| anyhow::anyhow!(e))
        } else {
            anyhow::bail!("Unknown contract name: {contract_name}");
        }
    }

    fn build_commitment_metadata(
        &self,
        contract_name: &ContractName,
        blob: &Blob,
    ) -> anyhow::Result<Vec<u8>> {
        if contract_name == &self.hydentity_name {
            self.hydentity
                .build_commitment_metadata(blob)
                .map_err(|e| anyhow::anyhow!(e))
        } else if contract_name == &self.hyllar_name {
            self.hyllar
                .build_commitment_metadata(blob)
                .map_err(|e| anyhow::anyhow!(e))
        } else {
            anyhow::bail!("Unknown contract name: {contract_name}");
        }
    }
}

pub async fn setup_hyllar(users: u32) -> Result<Hyllar> {
    let mut hyllar = Hyllar::default();

    // Create an entry for each users
    for n in 0..users {
        let ident = &format!("{n}.hyllar_test");
        hyllar
            .transfer(FAUCET_ID, ident, 0)
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    Ok(hyllar)
}

/// Create a new contract "hyllar_test" that already contains entries for each users
pub async fn setup(hyllar: Hyllar, url: String, verifier: String) -> Result<()> {
    let tx = BlobTransaction::new(
        Identity::new("hyle.hyle"),
        vec![RegisterContractAction {
            contract_name: "hyllar_test".into(),
            verifier: verifier.into(),
            program_id: hyle_contracts::HYLLAR_ID.to_vec().into(),
            state_commitment: hyllar.commit(),
        }
        .as_blob("hyle".into(), None, None)],
    );

    let mut client = codec_tcp_server::connect("loadtest_client".to_string(), url)
        .await
        .unwrap();
    client
        .send(TcpServerMessage::NewTx(tx.into()))
        .await
        .unwrap();

    Ok(())
}

pub async fn generate(users: u32, states: States) -> Result<(Vec<Transaction>, Vec<Transaction>)> {
    let blob_txs = generate_blobs_txs(users).await?;
    let proof_txs = generate_proof_txs(users, states).await?;

    Ok((blob_txs, proof_txs))
}

pub async fn generate_blobs_txs(users: u32) -> Result<Vec<Transaction>> {
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

                let msg: Transaction = BlobTransaction::new(identity, blobs).into();
                local_blob_txs.push(msg);
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

pub async fn generate_proof_txs(users: u32, state: States) -> Result<Vec<Transaction>> {
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
                    local_proof_txs.push(tx.into());
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

pub async fn send(
    url: String,
    blob_txs: Vec<Transaction>,
    proof_txs: Vec<Transaction>,
) -> Result<()> {
    send_blob_txs(url.clone(), blob_txs).await?;
    send_proof_txs(url, proof_txs).await?;
    Ok(())
}

pub fn load_blob_txs(users: u32) -> Result<Vec<Transaction>> {
    info!("Loading blob transactions");
    let blob_txs: Vec<Transaction> =
        borsh::from_slice(&std::fs::read(format!("blob_txs.{users}.bin"))?)
            .expect("failed to decode blob_txs.bin");

    Ok(blob_txs)
}

pub fn load_proof_txs(users: u32) -> Result<Vec<Transaction>> {
    info!("Loading proof transactions");
    let proof_txs: Vec<Transaction> =
        borsh::from_slice(&std::fs::read(format!("proof_txs.{users}.bin"))?)
            .expect("failed to decode proof_txs.bin");

    Ok(proof_txs)
}

pub async fn send_blob_txs(url: String, blob_txs: Vec<Transaction>) -> Result<()> {
    info!("Sending blob transactions");

    // Spin out a few tasks to send the transactions in parallel
    let mut tasks = tokio::task::JoinSet::new();
    let number_of_tasks = 20;
    let chunk_size = blob_txs.len().div_ceil(number_of_tasks);
    for chunk in blob_txs.chunks(chunk_size) {
        let chunk = chunk.to_vec();
        let url = url.clone();
        tasks.spawn(async move {
            let mut client = codec_tcp_server::connect("loadtest-blob-client".to_string(), url)
                .await
                .unwrap();
            for blob_tx in chunk.iter() {
                client
                    .send(TcpServerMessage::NewTx(blob_tx.clone()))
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

pub async fn send_proof_txs(url: String, proof_txs: Vec<Transaction>) -> Result<()> {
    info!("Sending proof transactions");

    let mut tasks = tokio::task::JoinSet::new();
    let number_of_tasks = 20;
    let chunk_size = proof_txs.len().div_ceil(number_of_tasks);
    for chunk in proof_txs.chunks(chunk_size) {
        let chunk = chunk.to_vec();
        let url = url.clone();
        tasks.spawn(async move {
            let mut client = codec_tcp_server::connect("loadtest-proof-client".to_string(), url)
                .await
                .unwrap();
            for blob_tx in chunk.iter() {
                client
                    .send(TcpServerMessage::NewTx(blob_tx.clone()))
                    .await
                    .unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            info!("Blob transactions sent: {:?}", chunk.len());
        });
    }

    tasks.join_all().await;

    info!("Proof transactions sent: {:?} total", proof_txs.len());

    Ok(())
}

pub fn get_current_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis()
}

pub async fn send_transaction<S: StateUpdater>(
    client: &NodeApiHttpClient,
    transaction: ProvableBlobTx,
    ctx: &mut TxExecutor<S>,
) -> TxHash {
    let identity = transaction.identity.clone();
    let blobs = transaction.blobs.clone();
    let tx_hash = loop {
        match client
            .send_tx_blob(&BlobTransaction::new(identity.clone(), blobs.clone()))
            .await
        {
            Ok(res) => break res,
            Err(e) => {
                tracing::warn!("Error when sending tx blob, waiting before retry {:?}", e);
                tokio::time::sleep(Duration::from_millis(200)).await
            }
        }
    };

    let provable_tx = ctx.process(transaction).unwrap();
    for proof in provable_tx.iter_prove() {
        let tx = proof.await.unwrap();
        loop {
            match client.send_tx_proof(&tx).await {
                Ok(_) => break,
                Err(e) => {
                    tracing::warn!("Error when sending tx blob, waiting before retry {:?}", e);
                    tokio::time::sleep(Duration::from_millis(200)).await
                }
            };
        }
    }
    tx_hash
}

pub async fn long_running_test(node_url: String, _indexer_url: String) -> Result<()> {
    loop {
        tracing::warn!("{}", node_url.clone());
        let mut client = NodeApiHttpClient::new(node_url.clone())?;
        client.api_key = Some("KEY_LOADTEST".to_string());
        // let indexer = IndexerApiHttpClient::new(indexer_url)?;
        // Generate a random number of iterations for a generated hydentity + hyllar
        let rand_iterations = get_current_timestamp_ms() % 1000;

        // Setup random hyllar contract
        let rand = get_current_timestamp_ms() % 100000;
        let random_hyllar_contract: ContractName = format!("hyllar_{}", rand).into();
        let random_hydentity_contract: ContractName = format!("hydentity_{}", rand).into();
        let tx = BlobTransaction::new(
            Identity::new("hyle.hyle"),
            vec![RegisterContractAction {
                contract_name: random_hyllar_contract.clone(),
                verifier: hyle_contract_sdk::Verifier("risc0-1".to_string()),
                program_id: hyle_contracts::HYLLAR_ID.to_vec().into(),
                state_commitment: Hyllar::custom(format!("faucet.{}", random_hydentity_contract))
                    .commit(),
            }
            .as_blob("hyle".into(), None, None)],
        );

        client.send_tx_blob(&tx).await?;

        let tx = BlobTransaction::new(
            Identity::new("hyle.hyle"),
            vec![RegisterContractAction {
                contract_name: random_hydentity_contract.clone(),
                verifier: hyle_contract_sdk::Verifier("risc0-1".to_string()),
                program_id: hyle_contracts::HYDENTITY_ID.to_vec().into(),
                state_commitment: Hydentity::default().commit(),
            }
            .as_blob("hyle".into(), None, None)],
        );

        client.send_tx_blob(&tx).await?;

        tokio::time::sleep(Duration::from_secs(5)).await;

        let mut users: Vec<u128> = vec![];

        let mut tx_ctx = TxExecutorBuilder::new(CanonicalStates {
            hydentity: Hydentity::default(),
            hydentity_name: random_hydentity_contract.clone(),
            hyllar: Hyllar::custom(format!("faucet.{}", random_hydentity_contract)),
            hyllar_name: random_hyllar_contract.clone(),
        })
        // Replace prover binaries for non-reproducible mode.
        .with_prover(
            random_hydentity_contract.clone(),
            Risc0Prover::new(HYDENTITY_ELF),
        )
        .with_prover(random_hyllar_contract.clone(), Risc0Prover::new(HYLLAR_ELF))
        .build();

        let ident = Identity(format!("faucet.{}", random_hydentity_contract.0));

        // Register faucet identity
        let mut transaction = ProvableBlobTx::new(ident.clone());

        _ = register_identity(
            &mut transaction,
            random_hydentity_contract.clone(),
            "password".to_owned(),
        );

        let tx_hash = send_transaction(&client, transaction, &mut tx_ctx).await;
        tracing::info!("Register TX Hash: {}", tx_hash);

        for i in 1..rand_iterations {
            info!("Iteration {}", i);
            let now = get_current_timestamp_ms();

            // Create a new user
            if now % 5 == 0 || users.len() < 2 {
                let ident = Identity(format!("{}.{}", now, random_hydentity_contract.0));
                users.push(now);

                tracing::info!("Creating identity with 100 tokens: {}", ident);

                // Register new identity
                let mut transaction = ProvableBlobTx::new(ident.clone());

                _ = register_identity(
                    &mut transaction,
                    random_hydentity_contract.clone(),
                    "password".to_owned(),
                );

                let tx_hash = send_transaction(&client, transaction, &mut tx_ctx).await;
                tracing::info!("Register TX Hash: {}", tx_hash);

                // Feed with some token
                tracing::info!("Feeding identity {} with tokens", ident);

                let mut transaction =
                    ProvableBlobTx::new(format!("faucet.{}", random_hydentity_contract.0).into());

                verify_identity(
                    &mut transaction,
                    random_hydentity_contract.clone(),
                    &tx_ctx.hydentity,
                    "password".to_string(),
                )?;

                transfer(
                    &mut transaction,
                    random_hyllar_contract.clone(),
                    ident.0.clone(),
                    100,
                )?;

                let tx_hash = send_transaction(&client, transaction, &mut tx_ctx).await;
                tracing::info!("Transfer TX Hash: {}", tx_hash);

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

            let guy_1_id = format!("{}.{}", guy_1_id, random_hydentity_contract.0);
            let guy_2_id = format!("{}.{}", guy_2_id, random_hydentity_contract.0);

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
                random_hydentity_contract.clone(),
                &tx_ctx.hydentity,
                "password".to_string(),
            )?;
            transfer(
                &mut transaction,
                random_hyllar_contract.clone(),
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
            tracing::info!("Transfer TX Hash: {}", tx_hash);
        }
    }
}

pub async fn send_massive_blob(users: u32, url: String) -> Result<()> {
    let ident = Identity::new("test3.hydentity");

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
        let msg: Transaction = tx.into();
        txs.push(msg);
    }

    let mut client = codec_tcp_server::connect("loadtest-massive-client".to_string(), url)
        .await
        .unwrap();
    for encoded_blob_tx in txs.into_iter() {
        client
            .send(TcpServerMessage::NewTx(encoded_blob_tx))
            .await
            .unwrap();
    }

    Ok(())
}
