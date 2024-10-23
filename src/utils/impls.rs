use hyle_contract_sdk::BlobData;

use crate::model::{Blob, Fees};

impl Fees {
    pub fn default_test() -> Self {
        Self {
            payer: "payer".into(),
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
