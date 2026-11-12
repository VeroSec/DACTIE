#!/bin/bash
# Generates everything a deployment needs into build/: crypto material (cryptogen), the
# genesis block of channel "dactiebench" (configtxgen), the chaincode-as-a-service package
# and its ID, the 1 KiB MISP record of the DACTIE sweep, and the chaincode-server and
# load-generator images (built here, loaded on the hosts by deploy.sh).
# Usage: ./generate.sh hosts/local.env | hosts/minervas.env
set -euo pipefail
source "$(dirname "$0")/common.sh"
ENV_FILE=$(realpath "${1:?usage: generate.sh hosts/<name>.env}")
cd "$HERE"
load_env "$ENV_FILE"

rm -rf build
mkdir -p build/cc
cp "$ENV_FILE" build/hosts.env
envsubst < network/crypto-config.yaml.tmpl > build/crypto-config.yaml
envsubst < network/configtx.yaml.tmpl > build/configtx.yaml
tools() {
    docker run --rm $(local_docker_user) -v "$HERE/build:/work" -w /work -e FABRIC_CFG_PATH=/work \
        "hyperledger/fabric-tools:$FABRIC_VERSION" "$@"
}
tools cryptogen generate --config=crypto-config.yaml --output=crypto
tools configtxgen -profile ChannelUsingRaft -outputBlock channel.block -channelID dactiebench

# The package only tells a peer where its chaincode server listens: every peer host runs one
# at 127.0.0.1:9999 (network/compose.yaml, service cc).
echo '{"address": "127.0.0.1:9999", "dial_timeout": "10s", "tls_required": false}' > build/cc/connection.json
echo '{"type": "ccaas", "label": "dactiebench_1"}' > build/cc/metadata.json
tar -C build/cc -czf build/cc/code.tar.gz connection.json
tar -C build/cc -czf build/dactiebench.tgz metadata.json code.tar.gz
echo "CHAINCODE_ID=dactiebench_1:$(sha256sum build/dactiebench.tgz | cut -d' ' -f1)" > build/chaincode.env

cp ../../workloads/misp/0dc6a09d-e8d7-48fd-a3f7-f554afb8d1b3.1KiB.json build/misp-1KiB.json
docker build -q -t dactie-fabric-cc:1 chaincode >/dev/null
docker build -q -t dactie-fabric-loadgen:1 loadgen >/dev/null
docker save dactie-fabric-cc:1 dactie-fabric-loadgen:1 | gzip > build/images.tar.gz
echo "build/ ready for $(basename "$ENV_FILE"): $(cat build/chaincode.env)"
