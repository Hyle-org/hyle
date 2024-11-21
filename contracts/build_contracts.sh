#!/bin/bash
set -e

# check if script is ran from root directory
if ! grep -q 'name = "hyle"' Cargo.toml 2>/dev/null; then
    echo "Error: This script must be run from the root directory containing the Cargo.toml with package name 'hyle'."
    exit 1
fi


build_contract() {
    local CONTRACT_NAME=$1
    local MANIFEST_PATH=$2
    local IMG_OUTPUT_PATH=$3
    local TXT_OUTPUT_PATH=$4

    read IMAGE FILE_PATH <<< $(cargo risczero build --manifest-path $MANIFEST_PATH | grep ImageID | awk -F 'ImageID: | - ' '{gsub(/"/, "", $3); print $2, $3}')
    cp $FILE_PATH $IMG_OUTPUT_PATH
    echo $IMAGE > $TXT_OUTPUT_PATH

    echo "$CONTRACT_NAME contract built successfully"
    echo "Image ID: $IMAGE"
    echo "Image path: $IMG_OUTPUT_PATH"
    echo "Do not forget do regenerate the e2e proofs with tests/build_proofs.sh"
}

usage() {
    echo "Usage: $0 [hydentity|hyllar|all]"
    echo "  hydentity: Build only the Hydentity contract"
    echo "  hyllar: Build only the Hyllar contract"
    echo "  all: Build all contracts (default if no parameter is provided)"
    echo "  help: Show this help message"
}

if [[ "$1" == "help" ]]; then
    usage
    exit 0
fi

case "$1" in
    hydentity)
        build_contract "Hydentity" "contracts/hydentity/guest/Cargo.toml" "./contracts/hydentity/hydentity.img" "./contracts/hydentity/hydentity.txt"
        ;;
    hyllar)
        build_contract "Hyllar" "contracts/hyllar/guest/Cargo.toml" "./contracts/hyllar/hyllar.img" "./contracts/hyllar/hyllar.txt"
        ;;
    "" | all)
        build_contract "Hydentity" "contracts/hydentity/guest/Cargo.toml" "./contracts/hydentity/hydentity.img" "./contracts/hydentity/hydentity.txt"
        build_contract "Hyllar" "contracts/hyllar/guest/Cargo.toml" "./contracts/hyllar/hyllar.img" "./contracts/hyllar/hyllar.txt"
        ;;
    *)
        echo "Invalid option: $1"
        usage
        exit 1
        ;;
esac
