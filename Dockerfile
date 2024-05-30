ARG GO_VERSION="1.21"
ARG RUNNER_IMAGE="oven/bun:1"
ARG TAG_VERIFIERS

# --------------------------------------------------------
# Builder
# --------------------------------------------------------
FROM golang:${GO_VERSION}-alpine3.18 as builder

# Install minimum necessary dependencies
ENV PACKAGES curl make bash jq sed zsh
RUN apk add --no-cache $PACKAGES

WORKDIR /hyle

# Downoad go dependencies
COPY go.mod go.sum ./x/zktx/go.mod ./
RUN --mount=type=cache,target=/root/.cache/go-build \
    --mount=type=cache,target=/root/go/pkg/mod \
    go mod download

# Add source file
COPY . .

# TODO: Warning! Each time you restart, that's a new blockchain :eyes:
RUN make build && make init

# --------------------------------------------------------
# Verifier
# --------------------------------------------------------
FROM europe-west3-docker.pkg.dev/hyle-413414/hyle-docker/verifiers-for-hyle:$TAG_VERIFIERS as verifier
# --------------------------------------------------------
# Runner
# --------------------------------------------------------
FROM ${RUNNER_IMAGE}

WORKDIR /hyle

# TODO: Embed everything together in a better way
COPY --from=builder /hyle/hyled /hyle
COPY --from=builder /hyle/hyled-data /hyle/hyled-data
COPY --from=verifier /risc0-verifier /hyle/risc0-verifier
COPY --from=verifier /sp1-verifier /hyle/sp1-verifier
COPY --from=verifier /noir-verifier /hyle/noir-verifier

# Could be interesting to use the 'bundle build' artifacts here
# Not possible ATM: https://github.com/oven-sh/bun/issues/11446
# > bundle with esbuild not working as well cause we use wasm
RUN cd /hyle/noir-verifier && bun install --frozen-lockfile

EXPOSE 26657 1317 9090

CMD ["/hyle/hyled", "start"]
