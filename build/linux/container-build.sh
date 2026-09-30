#!/usr/bin/env bash
# Reproducible baseline on any Docker-capable Linux host. Native architecture.
set -euo pipefail
cd "$(dirname "$0")/../.."
version=${1:?usage: container-build.sh VERSION}
docker build --file build/linux/Dockerfile --tag ptrack-linux-builder .
mkdir -p target/linux-packages target/linux-cargo target/linux-home dist
docker run --rm --user "$(id -u):$(id -g)" \
  --volume "$PWD:/workspace" --env HOME=/workspace/target/linux-home \
  --env CARGO_HOME=/workspace/target/linux-cargo \
  --env CARGO_TARGET_DIR=/workspace/target/linux-packages \
  --env "PTRACK_PACKAGE_VERSION=$version" ptrack-linux-builder \
  bash -c 'mkdir -p "$HOME"; npm --prefix frontend ci && npm --prefix frontend run build && bash build/linux/package.sh "$PTRACK_PACKAGE_VERSION"'
