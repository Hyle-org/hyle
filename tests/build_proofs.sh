#!/bin/bash
set -e

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

hyrun --user faucet.hydentity --password password hydentity register faucet.hydentity

BLOB_TX_HASH=$(hyled blobs faucet.hydentity hydentity 00106661756365742e687964656e74697479  | grep Response | awk -F'"' '{ print $2}')
hyled proof $BLOB_TX_HASH 0 hydentity hydentity.risc0.proof 
mv hydentity.risc0.proof tests/proofs/register.hydentity.risc0.proof

sleep 4
hyrun --user faucet.hydentity --password password hyllar transfer bob.hydentity 100
mv hyllar.risc0.proof tests/proofs/transfer.hyllar.risc0.proof
mv hydentity.risc0.proof tests/proofs/transfer.hydentity.risc0.proof
