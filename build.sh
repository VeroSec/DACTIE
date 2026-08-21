#!/bin/bash
# Extra flags are passed straight through to docker build, e.g.
#   ./build.sh --no-cache
#   ./build.sh --pull --no-cache
set -uo pipefail
DOCKER_FLAGS=("$@")

#Generates Key Material
mkdir -p ./key_material/
# Only seed the bootstrap id if it is not already set, so a rebuild does not
# wipe the authority peer id that the compose services read from this file.
if [ ! -f ./key_material/boot_id.env ]; then
    echo "BOOT_PEER_ID=<insert_here>" > ./key_material/boot_id.env
fi
for i in {0..9}; do mkdir -p ./key_material/key_material_$i; done

#Builds Docker images
# The peer image is built with the bench feature: the BENCH load generator is
# compiled out otherwise, and BENCH_PUBLISH in the compose has no effect.
docker build -t dactie-peer      -f ./dactie-peer/Dockerfile      . --build-arg FEATURES=bench "${DOCKER_FLAGS[@]}"
docker build -t dactie-authority -f ./dactie-authority/Dockerfile . "${DOCKER_FLAGS[@]}"
docker build -t dactie-archive   -f ./dactie-archive/Dockerfile   . "${DOCKER_FLAGS[@]}"
