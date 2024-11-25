# CNS: Contract Name Service 

## Register a new contract

All contracts needs to have a registered identity on any existing identity provider

Genesis provider could be:
- "com", "fr", "eu"... restricted to existing internet domains (see section below)
- "hyle" open to anyone

A new contract "MyContract" could then have a name like 
- mycontract.hyle 
- mycontract.com 

To register a new contract owner has to 

1. Register a new identity "mycontract.hyle"
2. Send a RegisterContract transaction 

```rust 
pub struct RegisterContractTransaction {
    //pub owner: String,
    pub verifier: String,
    pub program_id: Vec<u8>,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}
```
3. Send a proof of ownership for that contract name  

```rust 
pub struct ProofTransaction {
    pub proof: ProofData,
    pub contract_name: ContractName,
}
```
Note:
 - `contract_name` of the `ProofTransaction` is the identity provider for the registered contract name.
 - `hyle_output` of this transaction should refer the `RegisterContractTransaction`

4. If this proof is valid, the `RegisterContractTransaction` is settled and the contract registered.


## For internet domains from DNS

To have addresses like `apple.com`, `hyle.eu`...

- Using official DNS certificate allows to delegate attribution to external authority
    -> This authority is centralized, not really in the "blockchain" mindset

- Can be re-attributed if provided a valid certificate 

## New identity provider 

A new identity provider is just a new contract like any other. The developper register its own 
contract under any existing provider. 

Example: Twitter wants to have its own identity provider, where its user can have an on-chain identity
given they have a twitter account.

They have `x.com` registered and a certificate.
They register they identity `x.com` and register a contract on this name (or a subdomain like `id.x.com`)
They new contract verifies user identity using twitter sso, and anyone can have its identity (e.g.) `hyle-org.x.com` 

The end-user, can have an identity provided by X, without the need of having a `.com` certificate.

Other example:
Bob, a random developper of a new game called "Doom" wants to enable its users to log-in using a password.
Bob register an identity `doom.hyle` because it's available. 
Bob register its game under `doom.hyle`. His contract is the game itself, but also its own identity provider (why not ?).

Any doom user can register using a password and have identity "xxKillerxx.doom.hyle".

This identity can be used in any other contract if the user wants to and can receive shitcoins/nft on its doom account 
if he wants to.

## Initial Hyle contract

Simple contract that use asymetric signature to validate identity (classic blockchains).

### Revoke identity

The `hyle` contract, can have a "Revoke" action on an identity to replace it by a new one. This action should be accepted
by a node only if it has a BLST signature of 2f+1 validators.

**Bad point**: for the `hyle` contract, the node has to parse the BlobData to check if it is not a "Revoke" action.
