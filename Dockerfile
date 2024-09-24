FROM rust:latest AS builder

RUN apt-get update && apt-get install musl-tools -y

## Install target platform (Cross-Compilation) --> Needed for Alpine
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /usr/src/hyle
COPY . /usr/src/hyle

# This is a dummy build to get the dependencies cached.
RUN cargo build --target x86_64-unknown-linux-musl --release

# RUNNER
FROM --platform=linux/amd64 alpine:latest

WORKDIR /usr/local/bin

COPY --from=builder /usr/src/hyle/target/x86_64-unknown-linux-musl/release/node ./
COPY ./master.ron ./config.ron

VOLUME /app/data

EXPOSE 4321 1234

# refers to the volume /app/data
ENV HYLE_DATA_DIRECTORY="data"

CMD ["node", "--config-file", "config.ron"]
