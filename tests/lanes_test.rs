#![allow(clippy::unwrap_used, clippy::expect_used)]

use hyle::log_error;

use anyhow::Result;
use fixtures::{ctx::E2ECtx, test_helpers::ConfMaker};
use hyle_model::{api::APIRegisterContract, ContractName, ProgramId, StateCommitment};

mod fixtures;

#[test_log::test(tokio::test)]
async fn lane_manager_outside_consensus() -> Result<()> {
    let mut ctx = E2ECtx::new_single_with_indexer(500).await?;

    _ = ctx.wait_height(1).await;

    let mut conf = ctx.make_conf("lane_mgr");
    conf.p2p.mode = hyle::utils::conf::P2pMode::LaneManager;
    // Remove indexer
    conf.p2p.peers.pop();
    let lane_mgr_client = ctx.add_node_with_conf(conf).await?;

    let tx_hash = lane_mgr_client
        .register_contract(&APIRegisterContract {
            verifier: "test".into(),
            program_id: ProgramId(vec![1, 2, 3]),
            state_commitment: StateCommitment(vec![1, 2, 3]),
            contract_name: ContractName::new("test"),
        })
        .await?;

    let indexer_client = ctx.indexer_client();

    loop {
        match indexer_client.get_transaction_with_hash(&tx_hash).await {
            Ok(_) => {
                break;
            }
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }

    assert_eq!(
        indexer_client
            .get_indexer_contract(&ContractName::new("test"))
            .await
            .unwrap()
            .program_id,
        vec![1, 2, 3]
    );

    Ok(())
}
