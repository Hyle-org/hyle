> `main` is the development branch.
> When building applications or running examples, use the [latest release](https://github.com/hyli-org/hyli/releases) instead.

# Hyli

[![Telegram Chat][tg-badge]][tg-url]
[![Build Status][actions-badge]][actions-url]
[![Code Coverage][codecov-badge]][codecov-url]
[![Twitter][twitter-badge]][twitter-url]

_Hyli is the new proof-powered L1 to build the next generation of apps onchain._

This repository hosts the **work-in-progress Rust client** for the [Hyli](https://hyli.org) chain.

**Current Status**: 🚧 Work in Progress (WIP)

---

## 📎 Useful Links

- 🌐 [Hyli Website](https://www.hyli.org/)
- 📚 [Hyli Documentation](https://docs.hyli.org)

---

## 🚀 Getting Started

For a single-node devnet (consensus disabled) with an indexer, clone the hyli repository and run:

```sh
cargo run -- --pg
```

This command starts a temporary PostgreSQL server and erases its data when you stop the node.

For alternative setups, optional features, and advanced configurations, check out the [devnet reference page](https://docs.hyli.org/reference/devnet/).

To write and deploy apps on the devnet, look at [our quickstart guide](https://docs.hyli.org/quickstart/)!

---

## 📊 Monitoring with Grafana and Prometheus

### Starting Services

To start the monitoring stack:

```bash
docker compose -f tools/docker-compose.yml up -d
```

### Access Grafana

Grafana is accessible at: [http://localhost:3000](http://localhost:3000)

### Stopping Services

To stop the monitoring stack:

```bash
docker compose -f tools/docker-compose.yml down
```

---

## 🛠️ Profiling and Debugging

### Profiling Build

Run the following command to enable the `profiling` profile, which is optimised but retains debug symbols:

```bash
cargo run --profile profiling
```

### CPU Profiling

- For advanced analysis, we recommend [Samply](https://github.com/mstange/samply).

### Memory Profiling

Hyli includes built-in support for the `dhat` crate, which uses the Valgrind DHAT viewer for memory profiling.  
To enable this feature, add the `dhat` feature flag. Use it selectively, as it has a runtime performance cost.

[actions-badge]: https://img.shields.io/github/actions/workflow/status/hyli-org/hyli/ci.yml?branch=main
[actions-url]: https://github.com/hyli-org/hyli/actions?query=workflow%3ATests+branch%3Amain
[codecov-badge]: https://codecov.io/gh/hyli-org/hyli/graph/badge.svg?token=S87GT99Q62
[codecov-url]: https://codecov.io/gh/hyli-org/hyli
[twitter-badge]: https://img.shields.io/twitter/follow/hyli_org
[twitter-url]: https://x.com/hyli_org
[tg-badge]: https://img.shields.io/endpoint?url=https%3A%2F%2Ftg.sumanjay.workers.dev%2Fhyli_org%2F&logo=telegram&label=chat&color=neon
[tg-url]: https://t.me/hyli_org
