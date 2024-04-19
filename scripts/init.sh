#!/bin/bash

# rm -r ~/.hyled || true
rm -r ./hyled-data || true
#HYLED_BIN=$(which hyled)
HYLED_BIN=./hyled
# configure hyled
$HYLED_BIN config set client chain-id hyle
$HYLED_BIN config set client keyring-backend test
$HYLED_BIN config set app minimum-gas-prices 0.1hyle
$HYLED_BIN keys add alice
$HYLED_BIN keys add bob
$HYLED_BIN init test --chain-id hyle --default-denom hyle
# update genesis
$HYLED_BIN genesis add-genesis-account alice 10000000hyle --keyring-backend test
$HYLED_BIN genesis add-genesis-account bob 1000hyle --keyring-backend test
# create default validator
$HYLED_BIN genesis gentx alice 1000000hyle --chain-id hyle
$HYLED_BIN genesis collect-gentxs

