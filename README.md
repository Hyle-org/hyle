# Hylé - your minimal layer one, focused only on verifying zero-knowledge proofs.

Repository for the [Hylé](https://hyle.eu) proof of concept chain.

Forked from [mini](https://github.com/cosmosregistry/chain-minimal) - the minimal Cosmos SDK chain.

**Current status**: proof of concept.

We plan to support all major proving schemes. Check [our list of currently supported proving systems](https://docs.hyle.eu/about/supported-proving-schemes/).

### Installation

Install and run:

```sh
git clone https://github.com/Hyle-org/hyle.git
cd hyle
make install or make build # builds the `hyled` binary
hyled --version # verify that the hyled command-line tool is installed and accessible
make init # initialize the chain
./hyled start # start the chain
```

## Useful links

* [Hylé website](https://www.hyle.eu/)
* [Hylé documentation](https://docs.hyle.eu)
* [Cosmos-SDK Documentation](https://docs.cosmos.network/)
