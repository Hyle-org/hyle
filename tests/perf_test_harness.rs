#![allow(clippy::unwrap_used, clippy::expect_used)]
use anyhow::Result;
use fixtures::ctx::E2ECtx;

mod fixtures;

#[ignore = "This is intended to easily start a few nodes locally for devs"]
#[test_log::test(tokio::test)]
async fn setup_4_nodes() -> Result<()> {
    let mut ctx = E2ECtx::new_multi_with_indexer(4, 1000).await?;

    // To use this harness, comment out the 'ignore' above and run something like:
    // RUST_LOG=warn cargo test --release --test perf_test_harness -- --nocapture

    ctx.stop_node(3).await?;
    ctx.get_instructions_for(3);

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}
