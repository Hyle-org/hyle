# Hyle Smart Contracts

This folder contains some "official" Hyle Risc0 smart contracts :

- `hydentity`: Basic identity provider
- `hyllar`: Simple ERC20-like contract
- `amm`: Simple AMM contract
- `risc0-recursion`: A contract with special rights to do recursion on multiple contracts 
- `staking`: A contract used to hold partg of the staking logic for the consensus.

This architecture is subject to change while sdk will be developped.

To regenerate the _.img and _.txt files, you should run

```
cargo build -p hyle-contracts --features build
```

For testing, you can recreate the contracts on the fly (without docker) by running the `--features nonreproducible` flag.
