# Fees in hyle
A proposal on how to handle fees on hyle

We assume some smart contracts existing from genesis block:
- `hyfi` to gather fees 
- `hydentity` to check identity

We assume a smart contract exists from genesis block to handle fees. We call it `hyfi`.
We consider a global account we call `network`, this is where users will send fees, and this account will be distributed among consensus validators.

## Hyfi contract 

state of contract: 
```rust
struct Hyfi {
    balances: HashMap<Identity, i64>,
    collected_fees: u64
}
```

methods of contract:
```rust 
/// `from` gives `amount` of its token to `collected_fees`
pub fn pay_fees(from: Identity, amount: u64);

/// Distribute all `collected_fees` to identities given its rate
pub fn distribute(to: Vec<(Identity, Rate)>);

/// Mint `amount` tokens to an identity
pub fn mint(to: Identity, amount: u64);

/// transfer between 2 identities 
pub fn transfer(from: Identity, to: Identity, amount: u64);
```

## Fees verification

*tldr*: Fees are checked *before* data dissemination, the node has to be sure that the network will be able to get the fees.

A Blob Transaction is:

```rust
pub struct IdentifiedBlobs {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
}

pub struct BlobTransaction {
    pub data: IdentifiedBlobs,
    pub fees: IdentifiedBlobs,
}
```

When a new transaction is sent to the node, it checks if the `fees` part contains:

- A blob for contract `hydentity` & identity is valid 
    - note: later we could consider a "whitelist" of contracts there
- A blob for contract `hyfi`
    - note: later we could consider a "whitelist" of contracts there
    - this is a call to method `pay_fees` from `fees.identity` 
    - `fees.identity` has enough balance

If all those are valid, the node knows it will be able to generate a proof for this blob and gather the fees. 
- ⚠️  Modulo there is no other transaction in an other batch that would be included before
    - if such a case happens, an `identity` can have a negative balance

🔎 We can "execute" the `hyfi` & `hydentity` smart contract code itself to check the output without proving it now.

If the transaction is valid, it can be sent to mempool for data dissemination.

❓ Should each node do this verification when receiving the tx in data dissemination ? If we don't, the network might accept transactions with no fees if a node "don't care" about fees in its data lane.
- Could be retro-actively slashed

❓ How to check balances with unsettled transactions ?
- Fees are taken independently of transaction settlement. The fee part will be settled by the node itself.
 
❓ How to take fees on blobs that can’t be prooved ?
- Fees are taken independently of transaction settlement. The fee part will be settled by the node itself.

❓ Fees are present in blob transaction, so they should pay for the proof transaction, which is the costly part. How can we anticipate the "cost" of the proof before having it, only with the blobs ?


## Fees gathering

Once the blob transaction is sequenced in a block, any node can generate the ProofTransaction for all fees blobs in the previous block, and once 
this one is disseminated & included in a block, all fees will be effectively moved to the `network` account.

```rust
pub struct ProofTransaction {
    pub blobs_references: Vec<BlobReference>,
    pub proof: Vec<u8>,
}
```
❓ If no nodes feels "responsible" of that, we could be in a situation where most nodes say "I don't need to do it, someone else will do it", and then it's always the same nodes 
that does this proof generation... How can we incentivize _all_ nodes to participates to this effort ?

## Fees distribution

Once fees blobs are settled, some funds are "locked" in the contract and can be distributed among validators that took part of the consensus. It does not need to be done at each block, it can be "delayed".
Whenever a node wants to take its profits, it can execute & proof the call of a function `distribute` on the contract `hyfi`. 

This function takes as input `validator_rates`: a list of validators, and a rate for each one, depending on how much it participated to the consensus since the last distribution.

This blob transaction needs to be checked by the consensus, to verify that the list `validator_rates` is valid, if not, the transaction should not be included in a bloc and the proposal rejected.

❓ Can we do this verification somewhere else ? It is mandatory that no one "sneaky" distribute fees with invalid list of validators rates.

