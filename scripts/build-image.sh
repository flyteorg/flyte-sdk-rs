#!/usr/bin/env bash
# Build + push the hello-trace worker image.
#
# Assembles a docker context containing this repo AND the rs_controller path
# dependency (../flyte-sdk/rs_controller), preserving the relative layout the
# Cargo path dep expects, then builds docker/Dockerfile for linux/amd64.
#
# Usage: IMAGE=ghcr.io/unionai/flyte-sdk-rs-demo:v1 ./scripts/build-image.sh [--push]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO_DIR="$(pwd)"
FLYTE_SDK_DIR="${FLYTE_SDK_DIR:-$REPO_DIR/../flyte-sdk}"
IMAGE="${IMAGE:?set IMAGE, e.g. ghcr.io/unionai/flyte-sdk-rs-demo:v1}"
PLATFORM="${PLATFORM:-linux/amd64}"

CTX="$(mktemp -d /tmp/flyte-sdk-rs-ctx.XXXXXX)"
trap 'rm -rf "$CTX"' EXIT
mkdir -p "$CTX/flyte-sdk" "$CTX/flyte-sdk-rust"
rsync -a --exclude target --exclude .git "$FLYTE_SDK_DIR/rs_controller" "$CTX/flyte-sdk/"
rsync -a --exclude target --exclude .git "$REPO_DIR/" "$CTX/flyte-sdk-rust/"

PUSH_FLAG="--load"
[ "${1:-}" = "--push" ] && PUSH_FLAG="--push"

docker buildx build \
  --platform "$PLATFORM" \
  -f "$REPO_DIR/docker/Dockerfile" \
  -t "$IMAGE" \
  $PUSH_FLAG \
  "$CTX"
echo "built $IMAGE ($PLATFORM)"
