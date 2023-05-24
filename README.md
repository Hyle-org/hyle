# Mini - A minimal Cosmos-SDK chain

This repository contains an example of a tiny, but working Cosmos-SDK chain.
It uses the least modules possible and is intended to be used as a starting point for learning build your own chain, without all the boilerplate that other tools generate. It can be seen as a simpler version of Cosmos-SDK's [simapp](https://github.com/cosmos/cosmos-sdk/tree/main/simapp).

`Minid` uses the **latest** version of the [Cosmos-SDK](https://github.com/cosmos/cosmos-sdk).

## Learn

**_Work in progress_**

Every bit of logic in this repository is commented in order to explain what it does and why do we add this.
Every folder contains a `README.md` file with a short description of the folder.

## How to use

In addition to learn how to build a chain thanks to `mini`, you can as well directly run `minid`.

### Installation

```sh
git clone git@github.com:julienrbrt/chain-minimal.git # pull this repository
cd chain-minimal
make install # install the minid binary
make init # initialize the chain
```

## Useful links

- [Cosmos-SDK Documentation](https://docs.cosmos.network/)
