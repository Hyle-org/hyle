use std::{collections::BTreeSet, hash::Hasher};

use borsh::{BorshDeserialize, BorshSerialize};
use rand::Rng;
use rand_seeder::SipHasher;
use sdk::{
    info,
    utils::{as_hyle_output, parse_raw_calldata},
    Blob, BlobData, BlobIndex, Calldata, ContractAction, ContractName, OnchainEffect, ProgramId,
    ProvableContractState, RegisterContractEffect, RunResult, StateCommitment, Verifier, ZkProgram,
};
use uuid::Uuid;

#[cfg(feature = "client")]
pub mod client;

#[derive(Clone, BorshSerialize, BorshDeserialize)]
pub struct UuidTldAction {
    pub verifier: Verifier,
    pub program_id: ProgramId,
    pub state_commitment: StateCommitment,
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

impl ZkProgram for UuidTld {
    fn execute(&mut self, calldata: &Calldata) -> RunResult {
        let (action, exec_ctx) = parse_raw_calldata::<UuidTldAction>(calldata)?;
        // Not an identity provider
        if calldata.identity.0.ends_with(&format!(
            ".{}",
            calldata.blobs[calldata.index.0].contract_name.0
        )) {
            return Err("Invalid identity".to_string());
        }

        let id = self.register_contract(calldata)?;

        Ok((
            format!("registered {}", id.clone()),
            exec_ctx,
            vec![OnchainEffect::RegisterContract(RegisterContractEffect {
                contract_name: format!(
                    "{}.{}",
                    id, calldata.blobs[calldata.index.0].contract_name.0
                )
                .into(),
                verifier: action.verifier,
                program_id: action.program_id,
                state_commitment: action.state_commitment,
            })],
        ))
    }

    fn commit(&self) -> sdk::StateCommitment {
        sdk::StateCommitment(self.serialize().unwrap())
    }
}

impl ProvableContractState for UuidTld {
    fn build_commitment_metadata(&self, _blob: &Blob) -> Result<Vec<u8>, String> {
        borsh::to_vec(self).map_err(|e| e.to_string())
    }
    fn execute_provable(&mut self, calldata: &Calldata) -> Result<sdk::HyleOutput, String> {
        let initial_state_commitment = <Self as ZkProgram>::commit(self);
        let mut res = <Self as ZkProgram>::execute(self, calldata);
        let next_state_commitment = <Self as ZkProgram>::commit(self);
        Ok(as_hyle_output(
            initial_state_commitment,
            next_state_commitment,
            calldata,
            &mut res,
        ))
    }
}

#[derive(Default, Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct UuidTld {
    registered_contracts: BTreeSet<u128>,
}

impl UuidTld {
    fn serialize(&self) -> Result<Vec<u8>, String> {
        borsh::to_vec(self).map_err(|e| e.to_string())
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, String> {
        borsh::from_slice(data).map_err(|e| e.to_string())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.serialize().expect("Failed to encode UuidTldState")
    }

    pub fn register_contract(&mut self, calldata: &Calldata) -> Result<Uuid, String> {
        let Some(ref tx_ctx) = calldata.tx_ctx else {
            return Err("Missing tx context".to_string());
        };

        // Create UUID
        let mut hasher = SipHasher::new();
        hasher.write(&self.commit().0);
        hasher.write(calldata.tx_hash.0.as_bytes());
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

#[cfg(test)]
mod test {
    use crate::*;
    use sdk::*;

    fn make_calldata(action: UuidTldAction) -> Calldata {
        Calldata {
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
            state_commitment: StateCommitment(vec![0, 1, 2, 3]),
        };
        let mut state = UuidTld::default();

        let calldata = make_calldata(action.clone());

        let (_, _, onchain_effects) = state.execute(&calldata).unwrap();

        let OnchainEffect::RegisterContract(effect) = onchain_effects.first().unwrap() else {
            panic!("Expected RegisterContract effect");
        };
        assert_eq!(
            effect.contract_name.0,
            "7de07efe-e91d-45f7-a5d2-0b813c1d3e10.uuid"
        );

        let calldata = make_calldata(action.clone());

        let (_, _, onchain_effects) = state.execute(&calldata).unwrap();

        let OnchainEffect::RegisterContract(effect) = onchain_effects.first().unwrap() else {
            panic!("Expected RegisterContract effect");
        };

        assert_eq!(
            effect.contract_name.0,
            "fe6d874b-7b90-496e-8328-1ea817be889a.uuid"
        );
    }
}
