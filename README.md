# Hylé

*A sequencing and settlement layer to help you build provable apps that are minimally, yet sufficiently, onchain.*

Repository for the [Hylé](https://hyle.eu) chain. This repository is for the work-in-progress rust client.
The older (but still maintained) Cosmos SDK based client can be found at [hyle-cosmos](https://github.com/Hyle-org/hyle-cosmos).

**Current status**: WIP

## Getting Started

### Build locally
```
  docker build . -t hyle_image:v1
  
```

### Run locally

```
  docker run -v ./db:/hyle/data -p 4321:4321 -p 1234:1234 hyle_image:v1 
```

If you have permission errors when accessing /hyle/data volume, use "--privileged" cli flag.

## Useful links

* [Hylé website](https://www.hyle.eu/)
* [Hylé documentation](https://docs.hyle.eu)
