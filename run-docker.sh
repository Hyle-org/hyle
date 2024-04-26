#!/usr/bin/env bash
set -e

rm -r ./hyled-data || true

NAME="hyle"
# TAG=${TAG:=$(git rev-parse --short HEAD)}
TAG="latest"
echo "Building docker image, tagging $TAG"
IMAGE_NAME="europe-west3-docker.pkg.dev/hyle-413414/hyle-docker/$NAME:$TAG"

docker build --platform linux/amd64 -t "$IMAGE_NAME" .
docker push "$IMAGE_NAME"
# Tag the image as 'latest' locally
docker tag "$IMAGE_NAME" "$NAME:latest"
# Cleanup
docker image rm "$IMAGE_NAME"
