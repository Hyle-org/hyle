FROM rust:alpine AS chef 
RUN apk add pkgconfig openssl-dev gcc musl-dev
RUN apk add --no-cache openssl-libs-static
RUN apk add curl bash
RUN cargo install cargo-chef 
WORKDIR /usr/src/hyle


FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY .cargo/config.toml .cargo/config.toml
COPY src ./src
COPY contract-sdk ./contract-sdk
COPY contracts ./contracts
COPY crates ./crates
RUN cargo chef prepare  --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /usr/src/hyle/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json
# Build application
COPY Cargo.toml Cargo.lock ./
COPY .cargo/config.toml .cargo/config.toml
COPY src ./src
COPY contract-sdk ./contract-sdk
COPY contracts ./contracts
COPY crates ./crates
RUN cargo build --bin node --bin indexer --release --features node_local_proving

# RUNNER
FROM alpine:latest

WORKDIR /hyle

COPY --from=builder /usr/src/hyle/target/release/node ./
COPY --from=builder /usr/src/hyle/target/release/indexer ./

# installing Barrenteberg CLI
RUN apk add --no-cache curl bash
ENV SHELL=/bin/bash
RUN curl -L https://raw.githubusercontent.com/AztecProtocol/aztec-packages/master/barretenberg/cpp/installation/install | bash
ENV PATH="/root/.bb:$PATH"
RUN bbup -v 0.41.0

VOLUME /hyle/data

EXPOSE 4321 1234

ENV HYLE_DATA_DIRECTORY="data"
ENV HYLE_RUN_INDEXER="false"

CMD ["./node"]
