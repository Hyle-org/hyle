# Hylé - verification layer for all zero knowledge proofs

Repository for the [Hylé](https://hyle.eu) proof of concept chain.
Forked from [mini](https://github.com/cosmosregistry/chain-minimal) - the minimal Cosmos SDK chain.

Current status: extreme POC

Proving systems supported:
 - [x] Risc Zero
 - [ ] Cairo
   - [ ] Starknet
 - [ ] SP1
 - [x] Groth16
 - [ ] PlonK

(We plan to support all major proving schemes)

### Installation

Install and run:

```sh
git clone git@github.com:hyle-org/hyle.git
cd hyle
make build # Build, currently builds `hyled`
make init # initialize the chain
./hyled start # start the chain
```

## Useful links

* [Cosmos-SDK Documentation](https://docs.cosmos.network/)

