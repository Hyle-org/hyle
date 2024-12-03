# Hylé

[![Build Status][actions-badge]][actions-url]
[![codecov][codecov-badge]][codecov-url]
[![Twitter][twitter-badge]][twitter-url]


_A sequencing and settlement layer to help you build provable apps that are minimally, yet sufficiently, onchain._

Repository for the [Hylé](https://hyle.eu) chain. This repository is for the work-in-progress rust client.
The older (but still maintained) Cosmos SDK based client can be found at [hyle-cosmos](https://github.com/Hyle-org/hyle-cosmos).

**Current status**: WIP

## Useful links

- [Hylé website](https://www.hyle.eu/)
- [Hylé documentation](https://docs.hyle.eu)

## Getting Started with Cargo

Start a single-node devnet, consensus is disabled, usefull to build & debug smart contracts:

```bash
cargo build
HYLE_RUN_INDEXER=false cargo run --bin node
```

If you want to run with indexer you will need a running postgres server in order to run the indexer
```bash
# For default conf:
docker run -d --rm --name pg_hyle -p 5432:5432 -e POSTGRES_PASSWORD=postgres postgres
```

### Configuration 

You can edit the configuration using env vars, or by a config file:
```bash
# Copy default config where you run the node. If file named "config.ron" is present, it will be loaded by node at startup.
cp ./src/utils/conf_defaults.ron config.ron
```

Examples of configuration by env var:
```bash
HYLE_RUN_INDEXER=false 
HYLE_CONSENSUS__SLOT_DURATION=100
```

## Getting Started with Docker

### Build locally

```bash
  docker build . -t hyle

```

### Run locally with Docker

```bash
  docker run -v ./db:/hyle/data -e HYLE_RUN_INDEXER=false -p 4321:4321 -p 1234:1234 hyle
```

If you have permission errors when accessing /hyle/data volume, use "--privileged" cli flag.

### Run locally with grafana and prometheus

#### Starting services

```bash
  docker compose -f tools/docker-compose.yml up -d
```

#### Interact with node 

To interact with the node, you can use `hyled`:

```bash
cargo run --bin hyled -- --help
```

You can install it for easy access: 
```bash
cargo install --path . --bin hyled
hyled --help
```

#### Access Grafana

```bash
  http://localhost:3000
```

#### Stopping

```bash
  docker compose -f tools/docker-compose.yml down
```

### Profiling and debugging

Run `cargo run --profile profiling` to enable the profiling profile, which is optimised but retains debug information.

#### CPU profiling

The `tokio-console` can be used for some simple debugging.

Otherwise, we recommend (samply)[https://github.com/mstange/samply].

#### Memory profiling

Hylé has built-in support for the `dhat` crate, which uses the valgrind dhat viewer for memory profiling.
This has a runtime performance cost, so should only be enabled when needed. The corresponding feature is `dhat`.


[actions-badge]: https://img.shields.io/github/actions/workflow/status/Hyle-org/hyle/ci.yml?branch=main
[actions-url]: https://github.com/Hyle-org/hyle/actions?query=workflow%3ATests+branch%3Amain
[codecov-badge]: https://codecov.io/gh/Hyle-org/hyle/graph/badge.svg?token=S87GT99Q62
[codecov-url]: https://codecov.io/gh/Hyle-org/hyle
[twitter-badge]: https://img.shields.io/twitter/follow/hyle_org
[twitter-url]: https://x.com/hyle_org
