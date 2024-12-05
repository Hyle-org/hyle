use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Ref, RefCell, RefMut};
use core::ops::{Deref, DerefMut};

use crate::Identity;
use crate::{Blob, StructuredBlob};
use bincode::{Decode, Encode};

use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Default)]
pub struct ExecutionState {
    pub callees_blobs: RefCell<Vec<Blob>>,
    pub caller: Identity,
}

pub struct CalleeBlob<'a>(pub Ref<'a, Vec<Blob>>);
pub struct MutCalleeBlob<'a>(pub RefMut<'a, Vec<Blob>>);

impl<'a> Deref for CalleeBlob<'a> {
    type Target = Vec<Blob>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> Deref for MutCalleeBlob<'a> {
    type Target = Vec<Blob>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<'a> DerefMut for MutCalleeBlob<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait CallerCallee {
    fn caller(&self) -> &Identity;
    fn callees_blobs(&self) -> CalleeBlob;
    fn mut_callees_blobs(&self) -> MutCalleeBlob;
}

pub trait ConnectToContract<T> {
    fn connect_to(&self, contract_name: String) -> Contract<T>;
}
pub struct Contract<'a, T> {
    contract_name: String,
    callees_blobs: &'a RefCell<Vec<Blob>>,
    phantom: core::marker::PhantomData<T>,
}

impl<'a, T: PartialEq + 'static> Contract<'a, T>
where
    StructuredBlob<T>: TryFrom<Blob>,
{
    pub fn new(callees_blobs: &'a RefCell<Vec<Blob>>, contract_name: String) -> Self {
        Contract::<'a, T> {
            contract_name,
            callees_blobs,
            phantom: core::marker::PhantomData,
        }
    }

    pub fn call(&self, action: T) -> Result<(), String> {
        let index = self.callees_blobs.borrow().iter().position(|blob| {
            if blob.contract_name.0 != self.contract_name {
                return false;
            };
            let Ok(blob) = StructuredBlob::try_from(blob.clone()) else {
                return false;
            };
            blob.data.parameters == action
        });

        match index {
            Some(index) => {
                self.callees_blobs.borrow_mut().remove(index);
                Ok(())
            }
            None => Err(alloc::format!(
                "Blob with contract name {} not found in callees",
                self.contract_name
            )),
        }
    }
}
