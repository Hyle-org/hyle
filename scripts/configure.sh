#!/bin/sh

HYLED_BIN=./hyled

# configure hyled
# Set our client for hyle + test keyring
$HYLED_BIN config set client chain-id hyle
$HYLED_BIN config set client keyring-backend test

$HYLED_BIN config set client node "https://rpc.testnet.hyle.eu:443"