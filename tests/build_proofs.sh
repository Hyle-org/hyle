#!/bin/bash
set -xe

# To run this file, you need a node with an indexer 
# The node needs to run in risc0 dev mode with env var "RISC0_DEV_MODE=1"

export RISC0_DEV_MODE=1

# check if script is ran from root directory
if ! grep -q 'name = "hyle"' Cargo.toml 2>/dev/null; then
    echo "Error: This script must be run from the root directory containing the Cargo.toml with package name 'hyle'."
    exit 1
fi

hyrun() {
  cargo run -p hyrun -- $@
}

hyled() {
  cargo run --bin hyled -- $@
}

hyrun --user bob.hydentity --password password hydentity register bob.hydentity

BLOB_TX_HASH=$(hyled blobs bob.hydentity hydentity 000d626f622e687964656e74697479  | grep Response | awk -F' ' '{ print $2}')
hyled proof $BLOB_TX_HASH hydentity hydentity.risc0.proof 
mv hydentity.risc0.proof tests/proofs/register.hydentity.risc0.proof

sleep 4
hyrun --user faucet.hydentity --password password --nonce 0 hyllar transfer bob.hydentity 100
mv hyllar.risc0.proof tests/proofs/transfer.hyllar.risc0.proof
mv hydentity.risc0.proof tests/proofs/transfer.hydentity.risc0.proof
