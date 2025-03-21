use anyhow::Result;
use hyle_model::{ConsensusProposalHash, ValidatorPublicKey};
use tracing::debug;

use super::{
    role_follower::{FollowerRole, Prepare},
    Consensus,
};

pub(super) trait RoleSync {
    fn on_sync_request(
        &mut self,
        sender: ValidatorPublicKey,
        proposal_hash: ConsensusProposalHash,
    ) -> Result<()>;
    fn on_sync_reply(&mut self, prepare: Prepare) -> Result<()>;
}

impl RoleSync for Consensus {
    /// When a validator receives a sync request from another validator, it will check if it has the prepare message in its buffer.
    /// If it has the prepare message, it will send the prepare message to the requesting validator.
    /// If it does not have the prepare message, it will ignore the request to avoid unnecessary network traffic.
    fn on_sync_request(
        &mut self,
        sender: ValidatorPublicKey,
        proposal_hash: ConsensusProposalHash,
    ) -> Result<()> {
        debug!(
            proposal_hash = %proposal_hash,
            "Got sync request from {}", sender);
        if let Some(prepare) = self.follower_state().buffered_prepares.get(&proposal_hash) {
            debug!(
                proposal_hash = %proposal_hash,
                "Sending prepare to {}", sender);
            let prepare = prepare.clone();
            self.send_net_message(sender, hyle_model::ConsensusNetMessage::SyncReply(prepare))?;
        };
        Ok(())
    }

    fn on_sync_reply(&mut self, prepare: Prepare) -> Result<()> {
        let (sender, proposal, ticket, view) = prepare;
        self.on_prepare(sender, proposal, ticket, view)
    }
}
