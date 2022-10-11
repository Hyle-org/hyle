#!/bin/bash

rm -r ~/.minid || true
MINID_BIN=$(which minid)
# configure minid
$MINID_BIN config chain-id demo
$MINID_BIN config keyring-backend test
$MINID_BIN keys add alice
$MINID_BIN keys add bob
$MINID_BIN init test --chain-id demo
# update genesis
$MINID_BIN add-genesis-account alice 10000000mini --keyring-backend test
$MINID_BIN add-genesis-account bob 1000mini --keyring-backend test
dasel put string -f ~/.minid/config/genesis.json '.app_state.staking.params.bond_denom' 'mini'
dasel put string -f ~/.minid/config/genesis.json '.app_state.mint.params.mint_denom' "mini"
# create default validator
$MINID_BIN gentx alice 1000000mini --chain-id demo
$MINID_BIN collect-gentxs