use anyhow::Result;

use crate::{model::SharedRunContext, utils::modules::Module};

use super::{
    consensus_bus_client::ConsensusBusClient, metrics::ConsensusMetrics, staking::Stake, Consensus,
    ConsensusStore, StateTag,
};

impl Module for Consensus {
    fn name() -> &'static str {
        "Consensus"
    }

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

        let mut consensus = Consensus {
            metrics,
            bus,
            file: Some(file),
            store,
            config: ctx.common.config.clone(),
            crypto: ctx.node.crypto.clone(),
        };

        if let Some(stake) = ctx
            .common
            .config
            .consensus
            .genesis_stakers
            .get(&ctx.common.config.id)
        {
            if matches!(consensus.store.bft_round_state.state_tag, StateTag::Genesis) {
                consensus.add_trusted_validator(
                    &ctx.node.crypto.validator_pubkey().clone(),
                    Stake { amount: *stake },
                )?;
            }
        } else {
            consensus.store.bft_round_state.state_tag = StateTag::Joining;
        }

        Ok(consensus)
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        _ = self.start_master(self.config.clone());
        self.start()
    }
}
