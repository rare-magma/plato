#! /bin/sh

set -euo pipefail

podman build -f builder.Dockerfile . -t localhost/plato-builder && podman run -v ./dist:/usr/src/plato/dist:z -v ./crates:/usr/src/plato/crates:ro,z localhost/plato-builder sh -c "./build.sh && ./dist.sh"
