# Hyle contract SDK

In this crate you will find some helpers function to build your Smart Contract on Hyle.

🚧 This is work in progress, all of this is subject to changes. See [examples](https://github.com/Hyle-org/examples) for usages.

# Model

Some of the models are defined in `hyle-model` crates, which is re-exported by this crate.

For example, you can do either of these two:
```rust 
use contract_sdk::ContractInput;
// or 
use hyle_model::ContractInput;
```

It allows you to only depends on crate `contract-sdk` in your contract's code.


# Entrypoints

The inputs of the zkVM should be of type ContractInput defined in
```rust
pub struct ContractInput {
    pub state: Vec<u8>,
    pub identity: Identity,
    pub tx_hash: TxHash,
    pub private_blob: BlobData,
    pub blobs: Vec<Blob>,
    pub index: BlobIndex,
}
```

In `guest.rs` you will find 2 functions that are entry points of smart contracts. You should call at least one of them at the beginning of your contract.

```rust 
pub fn init_raw<Action>(input: ContractInput) -> (ContractInput, Action)

pub fn init_with_caller<Action>(
    input: ContractInput,
) -> Result<(ContractInput, StructuredBlob<Action>, Identity), String>
```

At the end of your contract, you need to output a `HyleOutput`, you can use the helper in `utils.rs`:

```rust
pub fn as_hyle_output<State>(
    input: ContractInput,
    new_state: State,
    res: crate::RunResult,
) -> HyleOutput
```

# Helpers 

You can find in `erc20.rs` and `identity_prover.rs` some structs & traits used to help building contracts of token transfers & identity providing. 
These are only helpers to build new contracts, not required standards.
