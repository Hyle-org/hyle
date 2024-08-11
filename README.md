# Hylé - your minimal layer one, focused only on verifying zero-knowledge proofs.

Repository for the [Hylé](https://hyle.eu) proof of concept chain.

Forked from [mini](https://github.com/cosmosregistry/chain-minimal) - the minimal Cosmos SDK chain.

**Current status**: proof of concept.

We plan to support all major proving schemes. Check [our list of currently supported proving systems](https://docs.hyle.eu/roadmap/supported-proving-schemes/).

### Installation

##### Build from source:

```sh
mkdir hyle
cd hyle
# Installation of verifiers
git clone git@github.com:hyle-org/verifiers-for-hyle.git
cd verifiers-for-hyle
cargo build --release
```
```sh
cd ..
git clone git@github.com:hyle-org/hyle.git
cd hyle
make build # builds the `hyled` binary
make init # initialize the chain
make start # start the chain with paths for verifiers.
```

##### From docker :

> _soon to come_

## Useful links

* [Hylé website](https://www.hyle.eu/)
* [Hylé documentation](https://docs.hyle.eu)
* [Cosmos-SDK Documentation](https://docs.cosmos.network/)
