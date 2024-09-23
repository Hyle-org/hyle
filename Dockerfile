FROM rust:latest as builder
WORKDIR /app

RUN apt-get update
RUN apt-get install libclang-dev -y

COPY . ./
RUN cargo build --release --target x86_64-unknown-linux-gnu


# TODO: replace with alpine
FROM debian:latest

WORKDIR /app

COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/node ./
COPY --from=builder /app/master.ron ./config.ron

VOLUME /app/data

EXPOSE 4321
EXPOSE 1234

# refers to the volume /app/data
ENV HYLE_DATA_DIRECTORY="data"

CMD ["/app/node", "--config-file", "config.ron"]
