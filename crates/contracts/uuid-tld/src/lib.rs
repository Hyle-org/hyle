use std::{collections::BTreeSet, hash::Hasher};

use borsh::{BorshDeserialize, BorshSerialize};
use rand::Rng;
use rand_seeder::SipHasher;
use sdk::{
    caller::{CalleeBlobs, CallerCallee, ExecutionContext, MutCalleeBlobs},
    info, Blob, BlobData, BlobIndex, ContractAction, ContractInput, ContractName, Digestable,
    HyleContract, Identity, ProgramId, RegisterContractEffect, RunResult, StateDigest, Verifier,
};
use uuid::Uuid;

#[cfg(feature = "client")]
pub mod client;

#[derive(Clone, BorshSerialize, BorshDeserialize)]
pub struct UuidTldAction {
    pub verifier: Verifier,
    pub program_id: ProgramId,
    pub state_digest: StateDigest,
}

impl UuidTldAction {
    pub fn as_blob(&self, contract_name: ContractName) -> Blob {
        <Self as ContractAction>::as_blob(self, contract_name, None, None)
    }
}

impl ContractAction for UuidTldAction {
    fn as_blob(
        &self,
        contract_name: ContractName,
        _caller: Option<BlobIndex>,
        _callees: Option<Vec<BlobIndex>>,
    ) -> Blob {
        #[allow(clippy::expect_used)]
        Blob {
            contract_name,
            data: BlobData(borsh::to_vec(self).expect("failed to encode BlstSignatureBlob")),
        }
    }
}

pub struct UuidTldContract {
    pub exec_ctx: ExecutionContext,
    state: UuidTldState,
}

impl CallerCallee for UuidTldContract {
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

impl HyleContract<UuidTldState, UuidTldAction> for UuidTldContract {
    fn execute_action(
        &mut self,
        action: UuidTldAction,
        contract_input: &ContractInput,
    ) -> RunResult<UuidTldState>
    where
        Self: Sized,
    {
        // Not an identity provider
        if contract_input.identity.0.ends_with(&format!(
            ".{}",
            contract_input.blobs[contract_input.index.0].contract_name.0
        )) {
            return Err("Invalid identity".to_string());
        }

        let id = self.state.register_contract(contract_input)?;

        Ok((
            format!("registered {}", id.clone()),
            self.state.clone(),
            vec![RegisterContractEffect {
                contract_name: format!(
                    "{}.{}",
                    id, contract_input.blobs[contract_input.index.0].contract_name.0
                )
                .into(),
                verifier: action.verifier,
                program_id: action.program_id,
                state_digest: action.state_digest,
            }],
        ))
    }

    fn init(state: UuidTldState, exec_ctx: ExecutionContext) -> Self {
        UuidTldContract { state, exec_ctx }
    }

    fn state(self) -> UuidTldState {
        self.state
    }
}

#[derive(Default, Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct UuidTldState {
    registered_contracts: BTreeSet<u128>,
}

impl UuidTldState {
    fn serialize(&self) -> Result<Vec<u8>, String> {
        borsh::to_vec(self).map_err(|e| e.to_string())
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, String> {
        borsh::from_slice(data).map_err(|e| e.to_string())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.serialize().expect("Failed to encode UuidTldState")
    }

    pub fn register_contract(&mut self, contract_input: &ContractInput) -> Result<Uuid, String> {
        let Some(ref tx_ctx) = contract_input.tx_ctx else {
            return Err("Missing tx context".to_string());
        };

        // Create UUID
        let mut hasher = SipHasher::new();
        hasher.write(&self.as_digest().0);
        hasher.write(contract_input.tx_hash.0.as_bytes());
        hasher.write(tx_ctx.block_hash.0.as_bytes());
        hasher.write_u128(tx_ctx.timestamp);
        let mut hasher_rng = hasher.into_rng();
        let id = uuid::Builder::from_random_bytes(hasher_rng.random()).into_uuid();

        // _really_ shouldn't happen but let's handle it regardless.
        if !self.registered_contracts.insert(id.as_u128()) {
            return Err("Contract already registered".to_string());
        }

        info!("Registering new contract with UUID {}", id);

        Ok(id)
    }
}

impl Digestable for UuidTldState {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(self.serialize().unwrap())
    }
}

#[cfg(test)]
mod test {
    use crate::*;
    use sdk::*;

    fn make_contract_input(action: UuidTldAction, state: Vec<u8>) -> ContractInput {
        ContractInput {
            state,
            identity: "toto.test".into(),
            tx_hash: TxHash::default(),
            tx_ctx: Some(TxContext {
                block_hash: ConsensusProposalHash("0xcafefade".to_owned()),
                timestamp: 3745916,
                ..TxContext::default()
            }),
            private_input: vec![],
            blobs: vec![
                Blob {
                    contract_name: "test".into(),
                    data: BlobData(vec![]),
                },
                action.as_blob("uuid".into()),
            ],
            index: BlobIndex(1),
        }
    }

    #[test]
    fn test_execute() {
        let action = UuidTldAction {
            verifier: "test".into(),
            program_id: ProgramId(vec![1, 2, 3]),
            state_digest: StateDigest(vec![0, 1, 2, 3]),
        };
        let state = UuidTldState::default();

        let exec_ctx = ExecutionContext {
            caller: Identity::new("test"),
            ..ExecutionContext::default()
        };
        let mut contract = UuidTldContract::init(state.clone(), exec_ctx);
        let contract_input = make_contract_input(action.clone(), borsh::to_vec(&state).unwrap());

        let (_, state, registered_contracts) = contract
            .execute_action(action.clone(), &contract_input)
            .unwrap();

        let effect: &RegisterContractEffect = registered_contracts.first().unwrap();

        assert_eq!(
            effect.contract_name.0,
            "7de07efe-e91d-45f7-a5d2-0b813c1d3e10.uuid"
        );

        let contract_input = make_contract_input(action.clone(), borsh::to_vec(&state).unwrap());

        let (_, _, registered_contracts) =
            contract.execute_action(action, &contract_input).unwrap();

        let effect = registered_contracts.first().unwrap();

        assert_eq!(
            effect.contract_name.0,
            "fe6d874b-7b90-496e-8328-1ea817be889a.uuid"
        );
    }
}
