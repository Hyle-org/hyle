use anyhow::Result;

use crate::{model::SharedRunContext, utils::modules::Module};

use super::{
    api, consensus_bus_client::ConsensusBusClient, metrics::ConsensusMetrics, Consensus,
    ConsensusStore,
};

impl Module for Consensus {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let file = ctx
            .common
            .config
            .data_directory
            .clone()
            .join("consensus.bin");
        let store: ConsensusStore = Self::load_from_disk_or_default(file.as_path());
        let metrics = ConsensusMetrics::global(ctx.common.config.id.clone());
        let bus = ConsensusBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        let api = api::api(&ctx.common).await;
        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/consensus", api));
            }
        }

        Ok(Consensus {
            metrics,
            bus,
            file: Some(file),
            store,
            config: ctx.common.config.clone(),
            crypto: ctx.node.crypto.clone(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.wait_genesis()
    }
}
