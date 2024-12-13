use anyhow::Result;
use bincode::{Decode, Encode};
use model::{BlockHeight, ValidatorPublicKey};
use sdk::{
    caller::{CalleeBlobs, CallerCallee, ExecutionContext, MutCalleeBlobs},
    Blob, BlobData, BlobIndex, ContractName, Identity, StructuredBlobData,
};
use serde::{Deserialize, Serialize};
use state::OnChainState;

pub mod model;
pub mod state;

#[cfg(feature = "metadata")]
pub mod metadata {
    pub const STAKING_ELF: &[u8] = include_bytes!("../staking.img");
    pub const PROGRAM_ID: [u8; 32] = sdk::str_to_u8(include_str!("../staking.txt"));
}

#[derive(Encode, Decode, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RewardsClaim {
    block_heights: Vec<BlockHeight>,
}

/// Enum representing the actions that can be performed by the IdentityVerification contract.
#[derive(Encode, Decode, Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum StakingAction {
    Stake { amount: u128 },
    Delegate { validator: ValidatorPublicKey },
    Distribute { claim: RewardsClaim },
}

impl StakingAction {
    pub fn as_blob(
        self,
        contract_name: ContractName,
        caller: Option<BlobIndex>,
        callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        Blob {
            contract_name,
            data: BlobData::from(StructuredBlobData {
                caller,
                callees,
                parameters: self.clone(),
            }),
        }
    }
}

pub struct StakingContract {
    exec_ctx: ExecutionContext,
    state: state::Staking,
}

impl CallerCallee for StakingContract {
    fn caller(&self) -> &Identity {
        &self.exec_ctx.caller
    }
    fn callee_blobs(&self) -> CalleeBlobs {
        CalleeBlobs(self.exec_ctx.callees_blobs.borrow())
    }
    fn mut_callee_blobs(&self) -> MutCalleeBlobs {
        MutCalleeBlobs(self.exec_ctx.callees_blobs.borrow_mut())
    }
}

impl StakingContract {
    pub fn new(exec_ctx: ExecutionContext, state: state::Staking) -> Self {
        StakingContract { exec_ctx, state }
    }

    pub fn execute_action(&mut self, action: StakingAction) -> Result<String, String> {
        match action {
            StakingAction::Stake { amount } => self.state.stake(self.caller().clone(), amount),
            StakingAction::Delegate { validator } => {
                self.state.delegate_to(self.caller().clone(), validator)
            }
            StakingAction::Distribute { claim: _ } => todo!(),
        }
    }

    pub fn on_chain_state(&self) -> OnChainState {
        self.state.on_chain_state()
    }

    pub fn state(self) -> state::Staking {
        self.state
    }
}
