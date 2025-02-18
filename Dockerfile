ARG DEP_IMAGE=hyle-dep
ARG ALPINE_IMAGE=ghcr.io/hyle-org/alpine:main

FROM $DEP_IMAGE AS builder

# Build application
COPY Cargo.toml Cargo.lock ./
COPY .cargo/config.toml .cargo/config.toml
COPY src ./src
COPY crates ./crates
RUN cargo build --bin hyle --bin indexer --release -F node_local_proving -F sp1

# RUNNER
FROM $ALPINE_IMAGE 

WORKDIR /hyle

COPY --from=builder /usr/src/hyle/target/release/hyle ./
COPY --from=builder /usr/src/hyle/target/release/indexer ./


VOLUME /hyle/data

EXPOSE 4321 1234

ENV HYLE_DATA_DIRECTORY="data"
ENV HYLE_RUN_INDEXER="false"
ENV HYLE_REST=0.0.0.0:4321

CMD ["./hyle"]
