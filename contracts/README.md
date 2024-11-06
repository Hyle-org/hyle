# Hyle Smart Contracts

This folder contains some "official" Hyle Risc0 smart contracts :

- `hydentity`: Basic identity provider
- `hyllar`: Simple ERC20-like contract

There is also a tool called `hyrun` that allows to execute those contracts, generate proofs... 
`hyrun` tool is a "host" in the Risc0 world

Each contract has its guest code in a `guest` folder, this code is very similar from a contract to an other, 
the real logic of contracts is in `contract` folder.

This architecture is subject to change while sdk will be developped.

## Usage 

`hyrun` commands only prints `hyled` commands to execute, it does not really send data to the node. Only `hyled` does. 
```sh
# Init hyllar smart contract 
$ cargo run -p hyrun -- hyllar init 1000
hyled contract default risc0 3d606e1f76d5ecd280047a20f2bbc6458385b41ef8bab8440c8ac8f989dc663c hyllar fbe8030106666175636574fbe80300

# Init hydentity contract 
$ cargo run -p hyrun -- hydentity init
hyled contract default risc0 c88cf4139236ca964c1eeed27c8e264257359ff6957b2a87f4a8f21317062730 hydentity 00

# Register account "faucet" with password "toto"
$ cargo run -p hyrun -- --password toto hydentity register faucet
# send blob tx
hyled blobs user hydentity 0006666175636574 
# send proof
hyled proof $BLOB_TX_HASH 0 hydentity hydentity.risc0.proof

# Send some funds to new bob account
$ cargo run -p hyrun -- --user faucet --password toto hyllar transfer bob 10
# send blob 
hyled blobs faucet hydentity 01066661756365740100 hyllar 0203626f620a 
# send first proof 
hyled proof $BLOB_TX_HASH 1 hyllar hyllar.risc0.proof
# send second proof 
hyled proof $BLOB_TX_HASH 0 hydentity hydentity.risc0.proof

```
