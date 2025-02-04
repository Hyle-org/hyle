use std::{collections::BTreeSet, hash::Hasher};

use borsh::{BorshDeserialize, BorshSerialize};
use rand::Rng;
use rand_seeder::SipHasher;
use sdk::{
    info, Blob, BlobData, BlobIndex, ContractAction, ContractInput, ContractName, Digestable,
    ProgramId, RegisterContractEffect, RunResult, StateDigest, Verifier,
};
use uuid::Uuid;

#[cfg(feature = "client")]
pub mod client;

#[derive(Clone, BorshSerialize, BorshDeserialize)]
pub struct RegisterUuidContract {
    pub verifier: Verifier,
    pub program_id: ProgramId,
    pub state_digest: StateDigest,
}

impl RegisterUuidContract {
    pub fn as_blob(&self, contract_name: ContractName) -> Blob {
        <Self as ContractAction>::as_blob(self, contract_name, None, None)
    }
}

impl ContractAction for RegisterUuidContract {
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

#[derive(Default, Clone, BorshSerialize, BorshDeserialize)]
pub struct UuidTldState {
    registered_contracts: BTreeSet<u128>,
}

impl UuidTldState {
    fn serialize(&self) -> Result<Vec<u8>, String> {
        borsh::to_vec(self).map_err(|e| e.to_string())
    }

    fn deserialize(data: &[u8]) -> Result<Self, String> {
        borsh::from_slice(data).map_err(|e| e.to_string())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.serialize().expect("Failed to encode UuidTldState")
    }
}

impl Digestable for UuidTldState {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(self.serialize().unwrap())
    }
}

fn register_contract(input: &ContractInput) -> Result<(Uuid, UuidTldState), String> {
    let mut state = UuidTldState::deserialize(&input.private_input)?;

    // Check initial state
    if state.as_digest() != input.initial_state {
        return Err("State digest mismatch".to_string());
    }

    let Some(ref tx_ctx) = input.tx_ctx else {
        return Err("Missing tx context".to_string());
    };

    // Create UUID
    let mut hasher = SipHasher::new();
    hasher.write(&input.initial_state.0);
    hasher.write(input.tx_hash.0.as_bytes());
    hasher.write(tx_ctx.block_hash.0.as_bytes());
    hasher.write_u128(tx_ctx.timestamp);
    let mut hasher_rng = hasher.into_rng();
    let id = uuid::Builder::from_random_bytes(hasher_rng.random()).into_uuid();

    // _really_ shouldn't happen but let's handle it regardless.
    if !state.registered_contracts.insert(id.as_u128()) {
        return Err("Contract already registered".to_string());
    }

    info!("Registering new contract with UUID {}", id);

    Ok((id, state))
}

pub fn execute(contract_input: ContractInput) -> RunResult<UuidTldState> {
    let (input, parsed_blob) = sdk::guest::init_raw::<RegisterUuidContract>(contract_input);

    let parsed_blob = match parsed_blob {
        Some(v) => v,
        None => {
            return Err("Failed to parse input blob".to_string());
        }
    };

    // Not an identity provider
    if input
        .identity
        .0
        .ends_with(&format!(".{}", input.blobs[input.index.0].contract_name.0))
    {
        return Err("Invalid identity".to_string());
    }

    match register_contract(&input) {
        Ok((id, next_state)) => Ok((
            format!("registered {}", id.clone()),
            next_state,
            vec![RegisterContractEffect {
                contract_name: format!("{}.{}", id, input.blobs[input.index.0].contract_name.0)
                    .into(),
                verifier: parsed_blob.verifier,
                program_id: parsed_blob.program_id,
                state_digest: parsed_blob.state_digest,
            }],
        )),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod test {
    use crate::*;
    use sdk::*;

    fn make_contract_input(state: UuidTldState, action: RegisterUuidContract) -> ContractInput {
        ContractInput {
            initial_state: state.as_digest(),
            identity: "toto.test".into(),
            tx_hash: TxHash::default(),
            tx_ctx: Some(TxContext {
                block_hash: ConsensusProposalHash("0xcafefade".to_owned()),
                timestamp: 3745916,
                ..TxContext::default()
            }),
            private_input: state.serialize().unwrap(),
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
        let action = RegisterUuidContract {
            verifier: "test".into(),
            program_id: ProgramId(vec![1, 2, 3]),
            state_digest: StateDigest(vec![0, 1, 2, 3]),
        };
        let mut state = UuidTldState::default();

        let (_, new_state, registered_contracts) =
            execute(make_contract_input(state.clone(), action.clone())).unwrap();

        let effect = registered_contracts.first().unwrap();

        assert_eq!(
            effect.contract_name.0,
            "9be7e500-9398-46e2-a471-376bbe290517.uuid"
        );
        state
            .registered_contracts
            .insert(207234404638461129629594146217149400343);

        assert_eq!(new_state.as_digest(), state.as_digest());

        let (_, new_state, registered_contracts) =
            execute(make_contract_input(state.clone(), action)).unwrap();

        let effect = registered_contracts.first().unwrap();

        assert_eq!(
            effect.contract_name.0,
            "289f1568-9f12-4cc0-8d18-b0df040b34ca.uuid"
        );
        state
            .registered_contracts
            .insert(53995129251464490355800204076743603402);

        assert_eq!(new_state.as_digest(), state.as_digest());
    }
}
