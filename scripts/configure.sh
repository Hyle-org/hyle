#!/bin/sh

# Check if there's a hyled-data directory and ask if we should remove it
if [ -d "./hyled-data" ]; then
  echo "hyled-data directory already exists. Do you want to remove it? (y/n)"
  echo "(this will delete any local devnet data you have, so make sure to backup if needed)"
  read answer
  if [ "$answer" != "${answer#[Yy]}" ] ;then
    rm -r ./hyled-data
  else
    echo "Please remove/backup the hyled-data directory before running this script"
    exit 1
  fi
fi

echo "Configuring Hyled for public devnet..."

HYLED_BIN=./hyled

# configure hyled
# Set our client for hyle + test keyring
$HYLED_BIN config set client chain-id hyle
# For now we're using the `test` keyring backend in devnet.
echo "Warning: Using the test keyring backend for devnet. This is insecure and should not be used for an real value."
$HYLED_BIN config set client keyring-backend test

$HYLED_BIN keys add default

$HYLED_BIN config set client node "https://rpc.devnet.hyle.eu:443"

echo "...All done !"
