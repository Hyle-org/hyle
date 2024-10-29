use std::time::Duration;

use anyhow::Result;
use fixtures::{contracts::TestContract, ctx::E2ECtx};
use tokio::time::sleep;

mod fixtures;

#[test_log::test(tokio::test)]
async fn e2e_single_node_generates_blocks() -> Result<()> {
    let ctx = E2ECtx::new_single(50).await?;

    // TODO remove these 3 lines when the single node is fixed
    // at the moment, the single node needs 2 transactions to start generating blocks

    sleep(Duration::from_secs(2)).await;
    ctx.register_contract::<TestContract>("c1").await?;
    ctx.register_contract::<TestContract>("c2").await?;

    ctx.wait_height(20).await?;

    Ok(())
}
