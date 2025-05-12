#![allow(clippy::unwrap_used, clippy::expect_used)]
use fixtures::ctx::E2ECtx;

mod fixtures;

use anyhow::Result;

mod e2e_metrics {

    use tracing::info;

    use super::*;

    async fn poll_metrics(ctx: E2ECtx) -> Result<()> {
        let metrics = ctx.metrics().await?;
        info!("-- polled metrics {}", metrics);

        assert!(metrics.contains("receive_"));
        assert!(metrics.contains("send_"));
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn test_metrics_are_present_on_endpoint() -> Result<()> {
        let ctx = E2ECtx::new_single(500).await?;

        poll_metrics(ctx).await
    }
}
