#!/usr/bin/env bash

set -e

echo "Generating TS proto code"

cd proto
buf generate --template buf.gen.ts.yaml