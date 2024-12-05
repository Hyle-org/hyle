use anyhow::Result;
use hyle_contract_sdk::{
    erc20::{self, ERC20Action},
    Blob, BlobIndex, Identity, StructuredBlobData,
};
use hyllar::{HyllarToken, HyllarTokenContract};
use tracing::info;

use crate::model::BlobTransaction;

pub fn handle_hyllar(
    tx: &BlobTransaction,
    index: BlobIndex,
    state: HyllarToken,
) -> Result<HyllarToken> {
    let Blob {
        contract_name,
        data,
    } = tx.blobs.get(index.0 as usize).unwrap();

    let data: StructuredBlobData<ERC20Action> = data.clone().try_into()?;

    let caller: Identity = data
        .caller
        .map(|i| {
            tx.blobs
                .get(i.0 as usize)
                .unwrap()
                .contract_name
                .0
                .clone()
                .into()
        })
        .unwrap_or(tx.identity.clone());

    let mut contract = HyllarTokenContract::init(state, caller);
    let res = erc20::execute_action(&mut contract, data.parameters);
    info!("🚀 Executed {contract_name}: {res:?}");
    Ok(contract.state())
}
