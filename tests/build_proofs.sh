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
######################################################
# Here is the flow that this script is following:
# Register bob in hydentity

# Send 25 hyllar from faucet to bob
# Send 50 hyllar2 from faucet to bob

# Register new amm contract "amm2"

# Bob approves 100 hyllar to amm2
# Bob approves 100 hyllar2 to amm2

# Bob registers a new pair in amm
#    By sending 20 hyllar to amm
#    By sending 50 hyllar2 to amm

# Bob swaps 5 hyllar for 10 hyllar2
#    By sending 5 hyllar to amm
#    By sending 10 hyllar2 to bob (from amm)
######################################################

# Register bob in hydentity
hyrun --user bob.hydentity --password password hydentity register bob.hydentity
BLOB_TX_HASH=$(hyled blobs bob.hydentity hydentity 000d626f622e687964656e74697479  | grep Response | awk -F' ' '{ print $2}')
hyled proof $BLOB_TX_HASH hydentity 0.risc0.proof
mv 0.risc0.proof tests/proofs/register.bob.hydentity.risc0.proof

sleep 4
# Sending 25 hyllar from faucet to bob.hydentity
# This proof is used in both hyllar_test and amm_test
hyrun --user faucet.hydentity --password password --nonce 0 hyllar hyllar transfer bob.hydentity 25
BLOB_TX_HASH=$(hyled blobs faucet.hydentity hydentity 01106661756365742e687964656e7469747900 hyllar 0000020d626f622e687964656e7469747919  | grep Response | awk -F' ' '{ print $2}')
hyled proof $BLOB_TX_HASH hydentity 0.risc0.proof
hyled proof $BLOB_TX_HASH hyllar 1.risc0.proof
mv 0.risc0.proof tests/proofs/transfer.25-hyllar-to-bob.hydentity.risc0.proof
mv 1.risc0.proof tests/proofs/transfer.25-hyllar-to-bob.hyllar.risc0.proof

sleep 4
# Sending 50 hyllar2 from faucet to bob.
# This proof is only used in amm_test.
hyrun --user faucet.hydentity --password password --nonce 1 hyllar hyllar2 transfer bob.hydentity 50
BLOB_TX_HASH=$(hyled blobs faucet.hydentity hydentity 01106661756365742e687964656e7469747901 hyllar2 0000020d626f622e687964656e7469747932 | grep Response | awk -F' ' '{ print $2}')
hyled proof $BLOB_TX_HASH hydentity 0.risc0.proof
hyled proof $BLOB_TX_HASH hyllar2 1.risc0.proof
mv 0.risc0.proof tests/proofs/transfer.50-hyllar2-to-bob.hydentity.risc0.proof
mv 1.risc0.proof tests/proofs/transfer.50-hyllar2-to-bob.hyllar2.risc0.proof

# Register new amm contract "amm2"
hyled contract owner risc0 $(cat contracts/amm/amm.txt) amm2 00

sleep 4
# Bob approves 100 hyllar to amm2
hyrun --user bob.hydentity --password password --nonce 0 hyllar hyllar approve amm2 100
BLOB_TX_HASH=$(hyled blobs bob.hydentity hydentity 010d626f622e687964656e7469747900 hyllar 00000404616d6d3264 | grep Response | awk -F' ' '{ print $2}')
hyled proof $BLOB_TX_HASH hydentity 0.risc0.proof
hyled proof $BLOB_TX_HASH hyllar 1.risc0.proof
mv 0.risc0.proof tests/proofs/approve.100-hyllar.hydentity.risc0.proof
mv 1.risc0.proof tests/proofs/approve.100-hyllar.hyllar.risc0.proof

sleep 4
# Bob approves 100 hyllar2 to amm2
hyrun --user bob.hydentity --password password --nonce 1 hyllar hyllar2 approve amm2 100
BLOB_TX_HASH=$(hyled blobs bob.hydentity hydentity 010d626f622e687964656e7469747901 hyllar2 00000404616d6d3264 | grep Response | awk -F' ' '{ print $2}')
hyled proof $BLOB_TX_HASH hydentity 0.risc0.proof
hyled proof $BLOB_TX_HASH hyllar2 1.risc0.proof
mv 0.risc0.proof tests/proofs/approve.100-hyllar2.hydentity.risc0.proof
mv 1.risc0.proof tests/proofs/approve.100-hyllar2.hyllar2.risc0.proof

sleep 4
# Bob registers a new pair in amm
#    By sending 20 hyllar to amm
#    By sending 50 hyllar2 to amm
hyrun --user bob.hydentity --password password --nonce 2 amm amm2 new-pair hyllar hyllar2 20 50
BLOB_TX_HASH=$(hyled blobs bob.hydentity hydentity 010d626f622e687964656e7469747902 amm2 0001020203010668796c6c61720768796c6c6172321432 hyllar 010100030d626f622e687964656e7469747904616d6d3214 hyllar2 010100030d626f622e687964656e7469747904616d6d3232 | grep Response | awk -F' ' '{ print $2}')
hyled proof $BLOB_TX_HASH hydentity 0.risc0.proof
hyled proof $BLOB_TX_HASH amm2 1.risc0.proof
hyled proof $BLOB_TX_HASH hyllar 2.risc0.proof
hyled proof $BLOB_TX_HASH hyllar2 3.risc0.proof
mv 0.risc0.proof tests/proofs/new-pair-hyllar-hyllar2.hydentity.risc0.proof
mv 1.risc0.proof tests/proofs/new-pair-hyllar-hyllar2.amm.risc0.proof
mv 2.risc0.proof tests/proofs/transfer.20-hyllar-to-amm.hyllar.risc0.proof
mv 3.risc0.proof tests/proofs/transfer.50-hyllar2-to-amm.hyllar2.risc0.proof

sleep 4
# Bob swaps 5 hyllar for 10 hyllar2
#    By sending 5 hyllar to amm
#    By sending 10 hyllar2 to bob (from amm)
hyrun --user bob.hydentity --password password --nonce 3 amm amm2 swap-flow hyllar hyllar2 5 10
BLOB_TX_HASH=$(hyled blobs bob.hydentity hydentity 010d626f622e687964656e7469747903 amm2 0001020203000668796c6c61720768796c6c617232 hyllar 010100030d626f622e687964656e7469747904616d6d3205 hyllar2 010100020d626f622e687964656e746974790a | grep Response | awk -F' ' '{ print $2}')
hyled proof $BLOB_TX_HASH hydentity 0.risc0.proof
hyled proof $BLOB_TX_HASH amm2 1.risc0.proof
hyled proof $BLOB_TX_HASH hyllar 2.risc0.proof
hyled proof $BLOB_TX_HASH hyllar2 3.risc0.proof
mv 0.risc0.proof tests/proofs/swap.hydentity.risc0.proof
mv 1.risc0.proof tests/proofs/swap.hyllar.hyllar2.risc0.proof
mv 2.risc0.proof tests/proofs/transfer.5-hyllar-to-amm.hyllar.risc0.proof
mv 3.risc0.proof tests/proofs/transfer.10-hyllar2-to-bob.hyllar2.risc0.proof