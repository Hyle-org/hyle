# Hyle Smart Contracts

This folder contains some "official" Hyle Risc0 smart contracts :

- `hydentity`: Basic identity provider
- `hyllar`: Simple ERC20-like contract
- `amm`: Simple AMM contract

There is also a tool called `hyrun` in the `../crates` directory that allows to execute those contracts, generate proofs...
`hyrun` tool is a "host" in the Risc0 world

This architecture is subject to change while sdk will be developped.

To regenerate the _.img and _.txt files, you should run

```
cargo build -p hyle-contracts --features build
```

For testing, you can recreate the contracts on the fly (without docker) by running the `--features nonreproducible` flag.
