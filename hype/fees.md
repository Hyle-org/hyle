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
pub struct Fees {
    pub payer: Identity,
    pub fee: Blob,
    pub identity: Blob,
    // If the identity blob is from a "known" contract, the node will be able to verify the identity without the proof
    pub identity_proof: Option<Vec<u8>> 
}

pub struct BlobTransaction {
    pub fees: Fees,
    pub identity: Identity,
    pub blobs: Vec<Blob>,
}
```

When a new transaction is sent to the node, it runs the `hyfi` & `hydentity` smart contracts code with `fee` and `identity` blobs.

If the identity blob is from a "known" contract like `hydentity`, the node will be able to verify the identity without the proof, so it can only verify the blob, 
but for later implementation, if identity comes from an unknown identity, the proof needs to be verified by the node before adding the transaction to its lane.

➡️  For now we check that `fees.identity_contract == "hydentity" && fees.fee_contract == "hyfi"`, but later we could consider whitelists.

If the execution succeeds **and** resulting balance of `payer` is greater that `base_amount` (Cf next ⚠️ ), it can be sent to mempool for data dissemination. A proof will be generated later (unless it's given).

⚠️  At this stage, the node knows it will be able to generate a proof for this blob and gather the fees. Modulo there is no other transaction 
in an other batch that would be included before
    - if such a case happens, an `identity` can have a negative balance
    - to limit the "negative balance" issue, we can consider that an `identity` needs to always have at least `base_amount` of token. If the payer has a balance under this amount the fees won't 
be accepted, but if the payers got multiple transactions in different lanes that ended to pay more fees than allowed, the payer won't get a negative balance. The payer won't be able to pay for fees
until it gains some more tokens.
    - It does not 100% delete the "negative balance issue", unless this `base_amount` is high enough... This is an hyper parameter to find.

❓ Should each node do this verification when receiving the tx in data dissemination ? If we don't, the network might accept transactions with no fees if a node "don't care" about fees in its data lane.
- Could be retro-actively slashed
- See ⚠️  in `## Fees gathering` 

❓ How to check balances with unsettled transactions ?
- Fees are taken independently of transaction settlement. The fee part will be settled by the node itself.
 
❓ How to take fees on blobs that can’t be prooved ?
- Fees are taken independently of transaction settlement. The fee part will be settled by the node itself.

❓ Fees are present in blob transaction, so they should pay for the proof transaction, which is the costly part. How can we anticipate the "cost" of the proof before having it, only with the blobs ?


## Fees gathering

Once the blob transaction is sequenced in a block, any node can generate the FeeProofTransaction for all fees blobs in the previous block(s), and once 
this one is disseminated & included in a block, all fees will be effectively taken.
```rust
pub struct FeeProofTransaction {
    pub transactions: Vec<TxHash>,
    pub proof: Vec<u8>,
}
```

⚠️  If a transaction was introduced in a block by a node that didn't check the identity. This FeeProofTransaction won't be able to be generated, (unless this single transaction is removed 
from the `transactions` list, which makes the logic complexe where it shouldn't). That's why we need a guarantee that no transactions are sequenced if the identity is valid. To ensure that, all nodes needs to check `identity_proof` during data dissemination.

❓ If no nodes feels "responsible" of that, we could be in a situation where most nodes say "I don't need to do it, someone else will do it", and then it's always the same nodes 
that does this proof generation... How can we incentivize _all_ nodes to participates to this effort ?

## Fees distribution

Once fees blobs are settled, some funds are "locked" in the contract and can be distributed among validators that took part of the consensus. It does not need to be done at each block, it can be "delayed".
Whenever a node wants to take its profits, it can execute & proof the call of a function `distribute` on the contract `hyfi`. 

This function takes as input `validator_rates`: a list of validators, and a rate for each one, depending on how much it participated to the consensus since the last distribution.

This blob transaction needs to be checked by the consensus, to verify that the list `validator_rates` is valid, if not, the transaction should not be included in a bloc and the proposal rejected.

❓ Can we do this verification somewhere else ? It is mandatory that no one "sneaky" distribute fees with invalid list of validators rates.

