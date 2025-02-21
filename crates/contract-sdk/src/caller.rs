use alloc::string::String;
use alloc::vec::Vec;
use borsh::{BorshDeserialize, BorshSerialize};
use core::cell::{Ref, RefCell, RefMut};
use serde::{Deserialize, Serialize};

use hyle_model::{Blob, ContractName, Identity, StructuredBlob};

// Used to hide the implementation of the callees blobs.
pub struct CalleeBlobs<'a>(pub Ref<'a, Vec<Blob>>);
pub struct MutCalleeBlobs<'a>(pub RefMut<'a, Vec<Blob>>);

/// ExecutionContext provides an implementation of data for the CallerCallee trait
#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Default)]
pub struct ExecutionContext {
    pub callees_blobs: RefCell<Vec<Blob>>,
    pub caller: Identity,
    pub contract_name: ContractName,
}

impl ExecutionContext {
    pub fn caller(&self) -> &Identity {
        &self.caller
    }

    pub fn callee_blobs(&self) -> CalleeBlobs {
        CalleeBlobs(self.callees_blobs.borrow())
    }

    fn mut_callee_blobs(&self) -> MutCalleeBlobs {
        MutCalleeBlobs(self.callees_blobs.borrow_mut())
    }

    pub fn is_in_callee_blobs<U>(
        &self,
        contract_name: &ContractName,
        action: U,
    ) -> Result<(), String>
    where
        U: BorshDeserialize + PartialEq,
        StructuredBlob<U>: TryFrom<Blob>,
    {
        let index = self.callee_blobs().0.iter().position(|blob| {
            if &blob.contract_name != contract_name {
                return false;
            };
            let Ok(blob) = StructuredBlob::try_from(blob.clone()) else {
                return false;
            };
            blob.data.parameters == action
        });

        match index {
            Some(index) => {
                self.mut_callee_blobs().0.remove(index);
                Ok(())
            }
            None => Err(alloc::format!(
                "Blob with contract name {} not found in callees",
                contract_name
            )),
        }
    }
}
