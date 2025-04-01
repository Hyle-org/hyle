#![allow(clippy::all)]

mod fixtures;
#[cfg(feature = "turmoil")]
#[cfg(test)]
mod turmoil_tests {
    use std::time::Duration;

    use hyle::log_error;
    use hyle_model::{
        BlobTransaction, ContractAction, ContractName, ProgramId, RegisterContractAction,
        StateCommitment,
    };
    use rand::{rngs::StdRng, SeedableRng};
    use tracing::warn;

    use crate::fixtures::{test_helpers::wait_height, turmoil::ctx::TurmoilCtx};

    pub fn make_register_contract_tx(name: ContractName) -> BlobTransaction {
        BlobTransaction::new(
            "hyle.hyle",
            vec![RegisterContractAction {
                verifier: "test".into(),
                program_id: ProgramId(vec![]),
                state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                contract_name: name,
            }
            .as_blob("hyle".into(), None, None)],
        )
    }

    #[test_log::test]
    fn turmoil_test_leader_killed() -> anyhow::Result<()> {
        let rng = StdRng::seed_from_u64(123);
        let mut sim = hyle_net::turmoil::Builder::new()
            .simulation_duration(Duration::from_secs(100))
            // .fail_rate(0.9)
            .tick_duration(Duration::from_millis(100))
            .enable_tokio_io()
            .build_with_rng(Box::new(rng));

        sim.set_message_latency_curve(0.8);

        let ctx = TurmoilCtx::new_multi(4, 500)?;
        ctx.setup_simulation(&mut sim)?;

        let client = ctx.client();

        sim.client("client", async move {
            for i in 1..20 {
                _ = wait_height(&client, 1).await;

                let tx = make_register_contract_tx(format!("contract-{}", i).into());

                _ = log_error!(client.send_tx_blob(&tx).await, "Sending tx blob");
            }
            for i in 1..20 {
                let contract = ctx.get_contract(format!("contract-{}", i).as_str()).await?;
                assert_eq!(contract.name, format!("contract-{}", i).as_str().into());
            }

            Ok(())
        });

        let mut finished: bool;
        let mut iteration: usize = 0;

        loop {
            iteration += 1;
            finished = sim.step().unwrap();

            if iteration == 200 {
                warn!("RESTARTING node 2");
                sim.bounce("node-2");
            }

            if iteration == 300 {
                warn!("RESTARTING node 3");
                sim.bounce("node-3");
            }

            if finished {
                return Ok(());
            }
        }
    }
}
