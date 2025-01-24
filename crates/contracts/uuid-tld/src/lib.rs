use std::{collections::BTreeSet, hash::Hasher};

use bincode::{Decode, Encode};
use rand::Rng;
use rand_seeder::SipHasher;
use sdk::{
    flatten_blobs, info, utils::as_hyle_output, Blob, BlobData, BlobIndex, ContractAction,
    ContractInput, ContractName, Digestable, HyleOutput, ProgramId, RegisterContractEffect,
    StateDigest, Verifier,
};
use uuid::Uuid;

#[cfg(feature = "client")]
pub mod client;

#[derive(Clone, Encode, Decode)]
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
            data: BlobData(
                bincode::encode_to_vec(self, bincode::config::standard())
                    .expect("failed to encode BlstSignatureBlob"),
            ),
        }
    }
}

#[derive(Default, Clone, Encode, Decode)]
pub struct UuidTldState {
    registered_contracts: BTreeSet<u128>,
}

impl UuidTldState {
    fn serialize(&self) -> Result<Vec<u8>, String> {
        bincode::encode_to_vec(self, bincode::config::standard()).map_err(|e| e.to_string())
    }

    fn deserialize(data: &[u8]) -> Result<Self, String> {
        bincode::decode_from_slice(data, bincode::config::standard())
            .map(|(v, _)| v)
            .map_err(|e| e.to_string())
    }
}

impl Digestable for UuidTldState {
    fn as_digest(&self) -> sdk::StateDigest {
        sdk::StateDigest(self.serialize().unwrap())
    }
}

fn register_contract(input: &ContractInput) -> Result<(Uuid, StateDigest), String> {
    let mut state = UuidTldState::deserialize(&input.private_blob.0)?;

    // Check initial state
    if state.as_digest() != input.initial_state {
        return Err("State digest mismatch".to_string());
    }

    // Create UUID
    let mut hasher = SipHasher::new();
    hasher.write(&input.initial_state.0);
    hasher.write(input.identity.0.as_bytes());
    hasher.write(input.tx_hash.0.as_bytes());
    let mut hasher_rng = hasher.into_rng();
    let id = uuid::Builder::from_random_bytes(hasher_rng.gen()).into_uuid();

    // _really_ shouldn't happen but let's handle it regardless.
    if !state.registered_contracts.insert(id.as_u128()) {
        return Err("Contract already registered".to_string());
    }

    info!("Registering new contract with UUID {}", id);

    Ok((id, state.as_digest()))
}

pub fn execute(contract_input: ContractInput) -> HyleOutput {
    let (input, parsed_blob) = sdk::guest::init_raw::<RegisterUuidContract>(contract_input);

    // Not an identity provider
    if input
        .identity
        .0
        .ends_with(&format!(".{}", input.blobs[input.index.0].contract_name.0))
    {
        return as_hyle_output(
            input.clone(),
            input.initial_state,
            Err("Invalid identity".to_string()),
        );
    }

    match register_contract(&input) {
        Ok((id, next_state)) => HyleOutput {
            version: 1,
            initial_state: input.initial_state,
            next_state,
            identity: input.identity,
            tx_hash: input.tx_hash,
            index: input.index.clone(),
            blobs: flatten_blobs(&input.blobs),
            success: true,
            program_outputs: bincode::encode_to_vec(
                RegisterContractEffect {
                    contract_name: format!("{}.{}", id, input.blobs[input.index.0].contract_name.0)
                        .into(),
                    verifier: parsed_blob.verifier,
                    program_id: parsed_blob.program_id,
                    state_digest: parsed_blob.state_digest,
                },
                bincode::config::standard(),
            )
            .unwrap(),
        },
        Err(e) => as_hyle_output(input.clone(), input.initial_state, Err(e)),
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
            private_blob: BlobData(state.serialize().unwrap()),
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

        let output = execute(make_contract_input(state.clone(), action.clone()));

        let (effect, _): (RegisterContractEffect, _) =
            bincode::decode_from_slice(&output.program_outputs, bincode::config::standard())
                .expect("failed to decode RegisterContractEffect");

        assert!(output.success);

        assert_eq!(
            effect.contract_name.0,
            "245cb6ab-0d06-45c9-9a50-238221f6090d.uuid"
        );
        state
            .registered_contracts
            .insert(48333604109836718354378223739540474125);

        assert_eq!(output.next_state.0, state.as_digest().0);

        let output = execute(make_contract_input(state.clone(), action));

        let (effect, _): (RegisterContractEffect, _) =
            bincode::decode_from_slice(&output.program_outputs, bincode::config::standard())
                .expect("failed to decode RegisterContractEffect");

        assert!(output.success);

        assert_eq!(
            effect.contract_name.0,
            "bf056af4-bb5a-4ff1-a089-ecc9dd2a01da.uuid"
        );
        state
            .registered_contracts
            .insert(253910678004284124104875462641404477914);

        assert_eq!(output.next_state.0, state.as_digest().0);
    }
}
