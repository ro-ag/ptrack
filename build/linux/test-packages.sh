#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
version=${1:?usage: test-packages.sh VERSION [DISTRO...]}
shift
case "$(uname -m)" in
  x86_64) arch=amd64 ;;
  aarch64) arch=arm64 ;;
  *) exit 1 ;;
esac
if [ "$#" = 0 ]; then
  set -- ubuntu:22.04 ubuntu:24.04 ubuntu:26.04 debian:13 fedora:44
fi
for distro in "$@"; do
  docker run --rm --shm-size=512m \
    --volume "$PWD/dist:/packages:ro" --volume "$PWD/build/linux:/scripts:ro" \
    "$distro" bash /scripts/smoke.sh "$version" "$arch"
done
