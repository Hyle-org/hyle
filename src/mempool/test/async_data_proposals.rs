use std::sync::atomic::AtomicI32;
use std::sync::Arc;

use anyhow::{bail, Result};
use assertables::assert_ok;
use hyle_contract_sdk::{
    flatten_blobs, Blob, BlobData, HyleOutput, Identity, ProgramId, StateDigest,
};
use tracing::info;

use crate::bus::command_response::Query;
use crate::bus::{bus_client, BusClientReceiver, BusClientSender};
use crate::consensus::{ConsensusEvent, ConsensusProposal};
use crate::data_availability::DataEvent;
use crate::genesis::{Genesis, GenesisEvent};
use crate::mempool::QueryNewCut;
use crate::mempool::{DataProposal, RestApiMessage};
use crate::model::mempool::Cut;
use crate::model::{BlobTransaction, ContractName, Hashable, SignedBlock, TransactionData};
use crate::model::{ProofTransaction, RegisterContractTransaction};
use crate::utils::crypto::AggregateSignature;
use crate::utils::integration_test::NodeIntegrationCtxBuilder;

bus_client! {
    struct Client {
        sender(GenesisEvent),
        sender(RestApiMessage),
        sender(Query<QueryNewCut, Cut>),
        receiver(DataEvent),
        receiver(ConsensusEvent),
    }
}

// Need at least two worker threads for this test, as we want parallel execution.
#[test_log::test(tokio::test(flavor = "multi_thread", worker_threads = 2))]
async fn test_mempool_isnt_blocked_by_proof_verification() {
    assert_ok!(impl_test_mempool_isnt_blocked_by_proof_verification().await);
}

async fn impl_test_mempool_isnt_blocked_by_proof_verification() -> Result<()> {
    let mut node_modules = NodeIntegrationCtxBuilder::new().await;
    node_modules.conf.consensus.slot_duration = 200;

    let mut node_modules = node_modules.skip::<Genesis>().build().await?;

    let mut node_client = Client::new_from_bus(node_modules.bus.new_handle()).await;

    let contract_name = ContractName("test1".to_owned());

    node_client.send(GenesisEvent::GenesisBlock(SignedBlock {
        data_proposals: vec![(
            node_modules.crypto.validator_pubkey().clone(),
            vec![DataProposal {
                id: 0,
                parent_data_proposal_hash: None,
                txs: vec![RegisterContractTransaction {
                    owner: "test".to_string(),
                    verifier: "test-slow".into(),
                    program_id: ProgramId(vec![]),
                    state_digest: StateDigest(vec![0, 1, 2, 3]),
                    contract_name: contract_name.clone(),
                }
                .into()],
            }],
        )],
        certificate: AggregateSignature::default(),
        consensus_proposal: ConsensusProposal::default(),
    }))?;

    // Wait until we process the genesis block
    node_modules.wait_for_processed_genesis().await?;

    info!("Processed genesis block");

    // Test setup: count the number of commits during the slow proof verification
    // if we're blocking the consensus, this will be lower than expected.
    //let staking = node1.consensus_ctx.staking();
    let start_time = std::time::Instant::now();
    let mut counting_client = Client::new_from_bus(node_modules.bus.new_handle()).await;
    let count = Arc::new(AtomicI32::new(0));
    let count2 = count.clone();
    let counting_task = tokio::spawn(async move {
        loop {
            let _: ConsensusEvent = counting_client.recv().await.expect("Should get cut");
            count2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });

    // Now send the slow proof TX. Channels are ordered so these will be handled in order.
    let blob_tx = BlobTransaction {
        identity: Identity(format!("toto.{}", contract_name.0)),
        blobs: vec![Blob {
            contract_name: contract_name.clone(),
            data: BlobData(vec![]),
        }],
    };
    node_client.send(RestApiMessage::NewTx(blob_tx.clone().into()))?;

    // Send as many TXs as needed to hung all the workers if we were calling spawn
    for _ in 0..tokio::runtime::Handle::current().metrics().num_workers() {
        node_client.send(RestApiMessage::NewTx(
            ProofTransaction {
                contract_name: contract_name.clone(),
                proof: client_sdk::ProofData::Bytes(
                    serde_json::to_vec(&vec![HyleOutput {
                        success: true,
                        identity: blob_tx.identity.clone(),
                        blobs: flatten_blobs(&blob_tx.blobs),
                        tx_hash: blob_tx.hash(),
                        ..HyleOutput::default()
                    }])
                    .unwrap(),
                ),
            }
            .into(),
        ))?;
        info!("Sent new tx");
    }

    // Wait until we commit this TX
    loop {
        let cut: DataEvent = node_client.recv().await?;
        match cut {
            DataEvent::NewBlock(block) => {
                info!("Got block");
                if block.txs.iter().any(|tx| {
                    if let TransactionData::VerifiedProof(data) = &tx.transaction_data {
                        data.contract_name == contract_name
                    } else {
                        false
                    }
                }) {
                    break;
                }
            }
        }
    }

    counting_task.abort();
    counting_task.await.expect_err("Should abort counting task");

    info!(
        "Counted {} commits",
        count.load(std::sync::atomic::Ordering::SeqCst)
    );
    let expected_commits = (start_time.elapsed().as_millis()
        / node_modules.conf.consensus.slot_duration as u128) as i32;

    // Add a little bit of leeway
    if count.load(std::sync::atomic::Ordering::SeqCst) < expected_commits - 1 {
        bail!(
            "Should have more than {} commits, have {}",
            expected_commits,
            count.load(std::sync::atomic::Ordering::SeqCst)
        );
    }

    // TODO: test sending it to another node

    Ok(())
}
