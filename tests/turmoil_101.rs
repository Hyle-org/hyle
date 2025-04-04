#![allow(clippy::all)]
#![cfg(feature = "turmoil")]
#![cfg(test)]

mod fixtures;

use std::time::Duration;

use hyle::log_error;
use hyle_model::{
    BlobTransaction, ContractAction, ContractName, ProgramId, RegisterContractAction,
    StateCommitment,
};
use hyle_net::net::Sim;
use rand::{rngs::StdRng, SeedableRng};

use crate::fixtures::{test_helpers::wait_height, turmoil::TurmoilCtx};

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

macro_rules! turmoil_simple {
    ($seed:literal, $simulation:ident, $test:ident) => {
        paste::paste! {
        #[test_log::test]
            fn [<turmoil_ $simulation _ $seed _ $test>]() -> anyhow::Result<()> {
                tracing::info!("Starting test {} with seed {}", stringify!([<turmoil_ $simulation _ $seed _ $test>]), $seed);
                let rng = StdRng::seed_from_u64($seed);
                let mut sim = hyle_net::turmoil::Builder::new()
                    .simulation_duration(Duration::from_secs(100))
                    .tick_duration(Duration::from_millis(50))
                    .enable_tokio_io()
                    .build_with_rng(Box::new(rng));

                let mut ctx = TurmoilCtx::new_multi(4, 500, $seed, &mut sim)?;

                let mut other = ctx.clone();

                sim.client("client", async move {
                    _ = $test(&mut other).await?;
                    Ok(())
                });

                $simulation(&mut ctx, &mut sim)?;

                ctx.clean()?;

                Ok(())
            }
        }
    };

    ($seed_from:literal..=$seed_to:literal, $simulation:ident, $test:ident) => {
        seq_macro::seq!(SEED in $seed_from..=$seed_to {
            turmoil_simple!(SEED, $simulation, $test);
        });
    };
}

// turmoil_simple!(400..=420, simulation_basic, submit_10_contracts);
// turmoil_simple!(500..=520, simulation_hold, submit_10_contracts);
turmoil_simple!(500..=600, simulation_one_more_node, submit_10_contracts);

pub fn simulation_hold(ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    let mut finished: bool;

    let from = ctx.random(1, 4);
    let to = loop {
        let candidate = ctx.random(1, 4);
        if candidate != from {
            break candidate;
        }
    };

    let when = ctx.random(5, 15);
    let duration = ctx.random(2, 10);

    tracing::info!(
        "Holding messages from {} to {} at {} for {} seconds",
        from,
        to,
        when,
        duration
    );

    loop {
        finished = sim.step().unwrap();

        let current_time = sim.elapsed();

        if current_time > Duration::from_secs(when)
            && current_time <= Duration::from_secs(when + duration)
        {
            sim.hold(ctx.conf(from).id, ctx.conf(to).id);
        }

        if current_time > Duration::from_secs(when + duration) {
            sim.release(ctx.conf(from).id, ctx.conf(to).id);
        }

        if finished {
            return Ok(());
        }
    }
}

pub fn simulation_one_more_node(ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    let mut finished: bool;

    let when = ctx.random(5, 15);

    let mut added_nodes = 0;

    loop {
        finished = sim.step().unwrap();

        let current_time = sim.elapsed();

        if current_time > Duration::from_secs(when) && added_nodes == 0 {
            added_nodes += 1;
            let client = ctx.add_node_to_simulation(sim)?;

            sim.client("client 2", async move {
                _ = wait_height(&client, 1).await;

                for i in 1..10 {
                    let contract = client
                        .get_contract(&format!("contract-{}", i).into())
                        .await?;
                    assert_eq!(contract.name.0, format!("contract-{}", i).as_str());
                }
                Ok(())
            })
        }

        if finished {
            return Ok(());
        }
    }
}
pub fn simulation_basic(_ctx: &mut TurmoilCtx, sim: &mut Sim<'_>) -> anyhow::Result<()> {
    _ = sim.run();
    Ok(())
}

pub async fn submit_10_contracts(ctx: &mut TurmoilCtx) -> anyhow::Result<()> {
    let client = ctx.client();

    _ = wait_height(&client, 1).await;

    for i in 1..10 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let tx = make_register_contract_tx(format!("contract-{}", i).into());

        _ = log_error!(client.send_tx_blob(&tx).await, "Sending tx blob");
    }
    for i in 1..10 {
        let contract = client
            .get_contract(&format!("contract-{}", i).into())
            .await?;
        assert_eq!(contract.name.0, format!("contract-{}", i).as_str());
    }

    Ok(())
}
