#!/bin/sh

rm -r ./hyled-data || true
HYLED_BIN=./hyled

# configure hyled

# Set our client for hyle + test keyring
$HYLED_BIN config set client chain-id hyle-devnet
$HYLED_BIN config set client keyring-backend test

# Gas prices are unconfigured by default, so we need to set them in order to be able to send transactions
$HYLED_BIN config set app minimum-gas-prices 0.1hyle
# Enable the GRPC
$HYLED_BIN config set app grpc.enable true
# Enable the REST API for the explorer
$HYLED_BIN config set app api.enable true
$HYLED_BIN config set app api.enabled-unsafe-cors true

# init validator
$HYLED_BIN init hyle-validator --chain-id hyle-devnet --default-denom hyle

# create default validator
$HYLED_BIN keys add alice
$HYLED_BIN genesis add-genesis-account alice 10000000hyle --keyring-backend test
$HYLED_BIN genesis gentx alice 1000000hyle --chain-id hyle

# Setup everything
$HYLED_BIN genesis collect-gentxs

# Allow CORS
find ./hyled-data/config/config.toml -type f -exec sed -i 's/cors_allowed_origins = \[\]/cors_allowed_origins = \["*"\]/g' {} \;

# Just run `$HYLED_BIN start` to start the node
