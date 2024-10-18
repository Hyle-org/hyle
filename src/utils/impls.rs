use crate::model::Fees;

impl Fees {
    pub fn default_test() -> Self {
        Self {
            payer: "payer".into(),
            amount: 0,
            fee_contract: "hyfi".into(),
            identity_contract: "hydentity".into(),
            identity_proof: vec![],
        }
    }
}
