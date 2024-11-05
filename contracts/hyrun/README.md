# Hyrun

`hyrun` is a tool to run hyle offical smart contracts and generate proofs.

You need to build the contracts guests with `build_contracts.sh` at the root dir of the repository.

Example usage (from git repo root dir):

```sh
# To start hyfi from a fresh new inital state use -i argument
cargo run -p hyrun -- -i mint bob 100
# Or to fetch hyfi current onchain state of contract :
cargo run -p hyrun -- mint bob 100
```

You can run `hyled` by using 
```sh 
cargo run --bin hyled 
```

Or you can install it:
```sh
cargo install --git https://github.com/Hyle-org/hyle.git --bin hyled
```
