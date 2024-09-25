FROM rust:latest AS builder

RUN apt-get update && apt-get install musl-tools -y

## Install target platform (Cross-Compilation) --> Needed for Alpine
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /usr/src/hyle
COPY Cargo.toml Cargo.lock .
COPY src ./src
COPY nocow ./nocow
COPY .cargo/config.toml .cargo/config.toml

# This is a dummy build to get the dependencies cached.
RUN cargo build --target x86_64-unknown-linux-musl --release

# RUNNER
FROM alpine:latest

WORKDIR /hyle

COPY --from=builder /usr/src/hyle/target/x86_64-unknown-linux-musl/release/node ./
COPY ./master.ron ./config.ron

VOLUME /hyle/data

EXPOSE 4321 1234

# refers to the volume /var/hyle-data
ENV HYLE_DATA_DIRECTORY="data"

CMD ["./node", "--config-file", "config.ron"]
