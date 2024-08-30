# Hylé

*A sequencing and settlement layer to help you build provable apps that are minimally, yet sufficiently, onchain.*

Repository for the [Hylé](https://hyle.eu) chain - this is currently a proof-of-concept implementation based on the Cosmos SDK.

Forked from [mini](https://github.com/cosmosregistry/chain-minimal) - the minimal Cosmos SDK chain.

**Current status**: proof of concept.

We plan to support all major proving schemes. Check [our list of currently supported proving systems](https://docs.hyle.eu/about/supported-proving-schemes/).

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

## Useful links

* [Hylé website](https://www.hyle.eu/)
* [Hylé documentation](https://docs.hyle.eu)
* [Cosmos-SDK Documentation](https://docs.cosmos.network/)
