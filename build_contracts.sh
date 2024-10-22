#!/bin/bash
set -e

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
}

usage() {
    echo "Usage: $0 [hyfi|hydentity|all]"
    echo "  hyfi: Build only the Hyfi contract"
    echo "  hydentity: Build only the Hydentity contract"
    echo "  all: Build both contracts (default if no parameter is provided)"
    echo "  help: Show this help message"
}

if [[ "$1" == "help" ]]; then
    usage
    exit 0
fi

case "$1" in
    hyfi)
        build_contract "Hyfi" "contracts/hyfi/guest/Cargo.toml" "./contracts/hyfi/hyfi.img" "./contracts/hyfi/hyfi.txt"
        ;;
    hydentity)
        build_contract "Hydentity" "contracts/hydentity/guest/Cargo.toml" "./contracts/hydentity/hydentity.img" "./contracts/hydentity/hydentity.txt"
        ;;
    "" | all)
        build_contract "Hyfi" "contracts/hyfi/guest/Cargo.toml" "./contracts/hyfi/hyfi.img" "./contracts/hyfi/hyfi.txt"
        build_contract "Hydentity" "contracts/hydentity/guest/Cargo.toml" "./contracts/hydentity/hydentity.img" "./contracts/hydentity/hydentity.txt"
        ;;
    *)
        echo "Invalid option: $1"
        usage
        exit 1
        ;;
esac

