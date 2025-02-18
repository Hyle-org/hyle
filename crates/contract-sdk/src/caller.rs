use alloc::string::String;
use alloc::vec::Vec;
use borsh::{BorshDeserialize, BorshSerialize};
use core::cell::{Ref, RefCell, RefMut};
use core::ops::{Deref, DerefMut};
use serde::{Deserialize, Serialize};

use hyle_model::{Blob, ContractName, Identity, StructuredBlob};

/// ExecutionContext provides an implementation of data for the CallerCallee trait
#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Default)]
pub struct ExecutionContext {
    pub callees_blobs: RefCell<Vec<Blob>>,
    pub caller: Identity,
    pub contract_name: ContractName,
}

// Used to hide the implementation of the callees blobs.
pub struct CalleeBlobs<'a>(pub Ref<'a, Vec<Blob>>);
pub struct MutCalleeBlobs<'a>(pub RefMut<'a, Vec<Blob>>);

impl Deref for CalleeBlobs<'_> {
    type Target = Vec<Blob>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for MutCalleeBlobs<'_> {
    type Target = Vec<Blob>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for MutCalleeBlobs<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// CallerCallee trait allows blobs to efficiently check
/// the presence and data from other blobs they called,
/// effectively implementing a call graph across blobs.
pub trait CallerCallee {
    fn caller(&self) -> &Identity;
    fn callee_blobs(&self) -> CalleeBlobs;
    fn mut_callee_blobs(&self) -> MutCalleeBlobs;
}

// Auto-implemented for all types implementing CallerCallee
pub trait CheckCalleeBlobs: CallerCallee {
    fn is_in_callee_blobs<U>(&self, contract_name: &ContractName, action: U) -> Result<(), String>
    where
        U: BorshDeserialize + PartialEq,
        StructuredBlob<U>: TryFrom<Blob>;
}

impl<T: CallerCallee> CheckCalleeBlobs for T {
    fn is_in_callee_blobs<U>(&self, contract_name: &ContractName, action: U) -> Result<(), String>
    where
        U: BorshDeserialize + PartialEq,
        StructuredBlob<U>: TryFrom<Blob>,
    {
        let index = self.callee_blobs().iter().position(|blob| {
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
                self.mut_callee_blobs().remove(index);
                Ok(())
            }
            None => Err(alloc::format!(
                "Blob with contract name {} not found in callees",
                contract_name
            )),
        }
    }
}
