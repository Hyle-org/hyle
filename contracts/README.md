# Hyle Smart Contracts

This folder contains some "official" Hyle Risc0 smart contracts :

- `hydentity`: Basic identity provider
- `hyfee`: Basic contract to handle fees 

There is also a tool called `hyrun` that allows to execute those contracts, generate proofs... 
`hyrun` tool is a "host" in the Risc0 world

Each contract has its guest code in a `guest` folder, this code is very similar from a contract to an other, 
the real logic of contracts is in `contract` folder.

This architecture is subject to change while sdk will be developped.