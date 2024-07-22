ARG GO_VERSION="1.21"
ARG RUNNER_IMAGE="oven/bun:1"

# --------------------------------------------------------
# Builder
# --------------------------------------------------------
FROM golang:${GO_VERSION}-alpine3.18 as node_builder

    # Install minimum necessary dependencies
    ENV PACKAGES curl make bash jq sed zsh
    RUN apk add --no-cache $PACKAGES

    WORKDIR /hyle

    # Downoad go dependencies
    COPY go.mod go.sum ./x/zktx/go.mod ./
    RUN --mount=type=cache,target=/root/.cache/go-build \
        --mount=type=cache,target=/root/go/pkg/mod \
        go mod download

    # Add source file
    COPY . .

    # TODO: Warning! Each time you restart, that's a new blockchain :eyes:
    RUN make build && make init

# --------------------------------------------------------
# Verifier
# --------------------------------------------------------

FROM rust:latest as verifiers_builder
    WORKDIR /app

    RUN apt-get update
    RUN apt-get install libclang-dev -y

    # install Go
    COPY --from=golang:1.22-alpine /usr/local/go/ /usr/local/go/
    ENV PATH="/usr/local/go/bin:${PATH}"

    COPY verifiers/Cargo.toml verifiers/Cargo.lock ./
    COPY verifiers/hyle-contract hyle-contract
    COPY verifiers/risc0-verifier risc0-verifier
    #COPY verifiers/sp1-verifier sp1-verifier
    COPY verifiers/midenvm-verifier midenvm-verifier
    COPY verifiers/cairo-verifier cairo-verifier
    RUN rustup override set nightly-2024-05-24
    RUN RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu

# --------------------------------------------------------
# Runner
# --------------------------------------------------------
FROM ${RUNNER_IMAGE}

    WORKDIR /hyle

    # TODO: Embed everything together in a better way
    COPY --from=node_builder /hyle/hyled /hyle
    COPY --from=node_builder /hyle/hyled-data /hyle/hyled-data
    COPY --from=verifiers_builder /app/target/x86_64-unknown-linux-gnu/release/risc0-verifier /hyle/risc0-verifier
    # COPY --from=verifiers_builder /app/target/x86_64-unknown-linux-gnu/release/sp1-verifier /hyle/sp1-verifier
    COPY --from=verifiers_builder /app/target/x86_64-unknown-linux-gnu/release/midenvm-verifier midenvm-verifier
    COPY --from=verifiers_builder /app/target/x86_64-unknown-linux-gnu/release/cairo-verifier /hyle/cairo-verifier

    COPY  verifiers/noir-verifier /hyle/noir-verifier

    # Could be interesting to use the 'bundle build' artifacts here
    # Not possible ATM: https://github.com/oven-sh/bun/issues/11446
    # > bundle with esbuild not working as well cause we use wasm
    RUN cd /hyle/noir-verifier && bun install --frozen-lockfile

    EXPOSE 26657 1317 9090

    CMD ["/hyle/hyled", "start"]
