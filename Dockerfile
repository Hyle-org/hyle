ARG DEP_IMAGE=hyle-dep
ARG ALPINE_IMAGE=ghcr.io/hyle-org/alpine:main

FROM $DEP_IMAGE AS builder

# Build application
COPY Cargo.toml Cargo.lock ./
COPY .cargo/config.toml .cargo/config.toml
COPY src ./src
COPY contract-sdk ./contract-sdk
COPY contracts ./contracts
COPY crates ./crates
RUN cargo build --bin node --bin indexer --release --features node_local_proving

# RUNNER
FROM $ALPINE_IMAGE 

WORKDIR /hyle

COPY --from=builder /usr/src/hyle/target/release/node ./
COPY --from=builder /usr/src/hyle/target/release/indexer ./


VOLUME /hyle/data

EXPOSE 4321 1234

ENV HYLE_DATA_DIRECTORY="data"
ENV HYLE_RUN_INDEXER="false"
ENV HYLE_REST=0.0.0.0:4321

CMD ["./node"]
