use anyhow::Result;

use crate::{
    model::{SharedRunContext, ValidatorPublicKey},
    utils::modules::Module,
};

use super::{
    consensus_bus_client::ConsensusBusClient, metrics::ConsensusMetrics, Consensus, ConsensusStore,
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
        let mut store: ConsensusStore = Self::load_from_disk_or_default(file.as_path());
        let metrics = ConsensusMetrics::global(ctx.common.config.id.clone());
        let bus = ConsensusBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        // FIXME a bit hacky for now
        if store.bft_round_state.leader_pubkey == ValidatorPublicKey::default() {
            store.bft_round_state.leader_index = 0;
            //store.bft_round_state.leader_id = "node-1".to_string(); FIXME
            if ctx.common.config.id == "node-1" {
                store.is_next_leader = true;
            }
        }

        Ok(Consensus {
            metrics,
            bus,
            genesis_pubkeys: vec![ctx.node.crypto.validator_pubkey().clone()],
            file: Some(file),
            store,
            config: ctx.common.config.clone(),
            crypto: ctx.node.crypto.clone(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        _ = self.start_master(self.config.clone());
        _ = self.start_timeout_checker(self.config.clone());
        self.start()
    }
}
