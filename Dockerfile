FROM rust:alpine AS builder

RUN apk add pkgconfig openssl-dev gcc musl-dev
RUN apk add --no-cache openssl-libs-static

WORKDIR /usr/src/hyle
COPY Cargo.toml Cargo.lock .
COPY src ./src
COPY .cargo/config.toml .cargo/config.toml

RUN cargo build --release

# RUNNER
FROM alpine:latest

WORKDIR /hyle

COPY --from=builder /usr/src/hyle/target/release/node ./
COPY ./master.ron ./config.ron

VOLUME /hyle/data

EXPOSE 4321 1234

# refers to the volume /var/hyle-data
ENV HYLE_DATA_DIRECTORY="data"

CMD ["./node", "--config-file", "config.ron"]
