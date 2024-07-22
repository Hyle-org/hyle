FROM rust:latest as builder
WORKDIR /app

RUN apt-get update
RUN apt-get install libclang-dev -y

# install Go
COPY --from=golang:1.22-alpine /usr/local/go/ /usr/local/go/
ENV PATH="/usr/local/go/bin:${PATH}"

COPY Cargo.toml Cargo.lock ./
COPY hyle-contract hyle-contract
COPY risc0-verifier risc0-verifier
#COPY sp1-verifier sp1-verifier
COPY midenvm-verifier midenvm-verifier
COPY cairo-verifier cairo-verifier
RUN rustup override set nightly-2024-05-24
RUN RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu

FROM alpine:latest
WORKDIR /
COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/risc0-verifier risc0-verifier
#COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/sp1-verifier sp1-verifier
COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/midenvm-verifier midenvm-verifier
COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/cairo-verifier cairo-verifier
COPY noir-verifier /noir-verifier