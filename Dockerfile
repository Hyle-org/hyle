FROM rust:alpine AS builder

RUN apk add pkgconfig openssl-dev gcc musl-dev
RUN apk add --no-cache openssl-libs-static

WORKDIR /usr/src/hyle

COPY Cargo.toml Cargo.lock .
COPY .cargo/config.toml .cargo/config.toml

COPY src ./src
COPY contract-sdk ./contract-sdk
COPY contracts ./contracts

RUN cargo build --bin node --bin indexer --release

# RUNNER
FROM alpine:latest

WORKDIR /hyle

COPY --from=builder /usr/src/hyle/target/release/node ./
COPY --from=builder /usr/src/hyle/target/release/indexer ./

VOLUME /hyle/data

EXPOSE 4321 1234

ENV HYLE_DATA_DIRECTORY="data"
ENV HYLE_RUN_INDEXER="false"

CMD ["./node"]
