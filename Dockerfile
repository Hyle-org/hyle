ARG DEP_IMAGE=hyle-dep
ARG BASE_IMAGE=ghcr.io/hyle-org/base:main

FROM $DEP_IMAGE AS builder

# Build application
COPY Cargo.toml Cargo.lock ./
COPY .cargo/config.toml .cargo/config.toml
COPY src ./src
COPY crates ./crates
RUN cargo build --bin hyle --bin indexer --bin hyle-loadtest --release -F sp1 -F risc0

# RUNNER
FROM $BASE_IMAGE 

WORKDIR /hyle

COPY --from=builder /usr/src/hyle/target/release/hyle ./
COPY --from=builder /usr/src/hyle/target/release/indexer ./
COPY --from=builder /usr/src/hyle/target/release/hyle-loadtest ./


VOLUME /hyle/data

EXPOSE 4321 1234

ENV HYLE_DATA_DIRECTORY="data"
ENV HYLE_RUN_INDEXER="false"
ENV HYLE_REST_ADDRESS=0.0.0.0:4321

CMD ["./hyle"]
