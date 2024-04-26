#!/bin/sh

rm -r ./hyled-data || true
HYLED_BIN=./hyled

source .env.test

# configure hyled

# Set our client for hyle + test keyring
$HYLED_BIN config set client chain-id hyle
$HYLED_BIN config set client keyring-backend test

# Gas prices are unconfigured by default, so we need to set them in order to be able to send transactions
$HYLED_BIN config set app minimum-gas-prices 0.1hyle
# Enable the GRPC
$HYLED_BIN config set app grpc.enable true
# Enable the REST API for the explorer
$HYLED_BIN config set app api.enable true
$HYLED_BIN config set app api.enabled-unsafe-cors true

# Change address for network compatibilities
$HYLED_BIN config set app grpc.address "0.0.0.0:9090"
$HYLED_BIN config set app api.address "tcp://0.0.0.0:1317"
$HYLED_BIN config set client node "tcp://0.0.0.0:26657"
find ./hyled-data/config/config.toml -type f -exec sed -i 's/127.0.0.1/0.0.0.0/g' {} \;
find ./hyled-data/config/config.toml -type f -exec sed -i 's/localhost/0.0.0.0/g' {} \;

# Allow cors
find ./hyled-data/config/config.toml -type f -exec sed -i 's/cors_allowed_origins = \[\]/cors_allowed_origins = \["*"\]/g' {} \;

$HYLED_BIN keys add alice
$HYLED_BIN keys add bob
# Add faucet's keys
echo "$FAUCET_MNEMONIC" | $HYLED_BIN keys add faucet -i

# init validator
echo "$NODE_MNEMONIC" | $HYLED_BIN init hyle-validator --chain-id hyle --default-denom hyle --recover

# update genesis
$HYLED_BIN genesis add-genesis-account alice 10000000hyle --keyring-backend test
$HYLED_BIN genesis add-genesis-account bob 1000hyle --keyring-backend test
# create default validator
$HYLED_BIN genesis gentx alice 1000000hyle --chain-id hyle
$HYLED_BIN genesis collect-gentxs
