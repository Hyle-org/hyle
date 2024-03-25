# Hylé - verification layer for all zero knowledge proofs

Repository for the [Hylé](https://hyle.eu) proof of concept chain.
Forked from [mini](https://github.com/cosmosregistry/chain-minimal) - the minimal Cosmos SDK chain.

Current status: extreme POC

Proving systems supported:
 - [x] Risc Zero
 - [ ] Cairo
   - [ ] Starknet
 - [ ] SP1
 - [ ] Groth16
 - [ ] PlonK

(We plan to support all major proving schemes)

### Installation

Install and run:

```sh
git clone git@github.com:hyle-org/hyle.git
cd hyle
make build # Build, currently builds `minid`
make init # initialize the chain
./minid start # start the chain
```

## Useful links

* [Cosmos-SDK Documentation](https://docs.cosmos.network/)

## ZKTX module

Sending initial transactions
./minid tx zktx execute mini1s35tpv67eafejyvpxxdtn4e7dgm8whmm07a2x6 b AA== AQ==  
./minid tx zktx execute mini1s35tpv67eafejyvpxxdtn4e7dgm8whmm07a2x6 b AQ== Aq==  

Querying state
./minid query zktx contract mini1s35tpv67eafejyvpxxdtn4e7dgm8whmm07a2x6
