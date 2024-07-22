# Verifiers for Hylé

This repository contains various programs to verify zero-knowledge proofs within the (Hylé blockchain)[http://github.com/hyle-org/hyle].

Structure:
- `hyle-contract` works as a minimal SDK, specifying required outputs for verifying ZK proofs.
- `midenvm-verifier` implements (WIP) a verifier for MASM. See (their doc)[https://0xpolygonmiden.github.io/miden-vm/intro/main.html]
- `noir-verifier` is a verifier for Noir/Barretenberg proofs, most used within the Aztec blockchain.
- `risc0-verifier` is used with RISC zero.
- `sp1-verifier`is used with SP1.

## Building

### Rust

Most of these projects use rust, so you'll need `rust` and `cargo` installed.

Simply run `cargo build --release` within the repository to compile everything

### Typescript

The noir verifier is a typescript project. We recommend using `bun` to run it. Installations instructions (here)[https://bun.sh]
There's no need to actually build it, but hylé expects `bun` to be in the path.

## Using within Hylé

If you've followed the above instructions, there's nothing left to do.
Otherwise, you'll need to setup the environment variables within Hylé to point to your executables.
