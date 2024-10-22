use hyle_contract_sdk::BlobData;

use crate::model::{Blob, Fees};

impl Fees {
    pub fn default_test() -> Self {
        Self {
            payer: "payer".into(),
            identity_proof: Some(vec![]),
            fee: Blob {
                contract_name: "hyfi".into(),
                data: BlobData(vec![]),
            },
            identity: Blob {
                contract_name: "hydentity".into(),
                data: BlobData(vec![]),
            },
        }
    }
}
