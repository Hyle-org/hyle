# Hylé

*A sequencing and settlement layer to help you build provable apps that are minimally, yet sufficiently, onchain.*

Repository for the [Hylé](https://hyle.eu) chain. This repository is for the work-in-progress rust client.
The older (but still maintained) Cosmos SDK based client can be found at [hyle-cosmos](https://github.com/Hyle-org/hyle-cosmos).

**Current status**: WIP

## Getting Started with Cargo

```bash
cargo build
cargo run --bin node
  
```

## Getting Started with Docker



### Build locally
```bash
  docker build . -t hyle_image:v1
  
```

### Run locally with Docker

```bash
  docker run -v ./db:/hyle/data -p 4321:4321 -p 1234:1234 hyle_image:v1 
```

If you have permission errors when accessing /hyle/data volume, use "--privileged" cli flag.

### Run locally with grafana and prometheus 

#### Starting services
```bash
  docker compose -f tools/docker-compose.yml up -d
```

#### Submit blob transaction

```bash
  curl -X POST --location 'http://localhost:4321/v1/tx/send/blob' \
--header 'Content-Type: application/json' \
--data '{
    "identity": "ident",
    "blobs": [
        {
            "contract_name": "contrat de test",
            "data": []
        }
    ]
}'
```

#### Access Grafana

```bash
  http://localhost:3000
```

#### Stopping
```bash
  docker compose -f tools/docker-compose.yml down
```




## Useful links

* [Hylé website](https://www.hyle.eu/)
* [Hylé documentation](https://docs.hyle.eu)
