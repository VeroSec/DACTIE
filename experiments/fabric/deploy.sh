#!/bin/bash
# Starts a fresh network from build/ (generate.sh) and sets up channel and chaincode.
# With ssh names in build/hosts.env (hosts/minervas.env), it first copies network/ and build/
# to REMOTE_DIR on every host, loads the chaincode and load-generator images and pulls the
# Fabric images each host needs. Without (hosts/local.env), everything runs here.
# Ledgers of an earlier deployment are deleted: the channel is created anew.
# Usage: ./deploy.sh
set -euo pipefail
source "$(dirname "$0")/common.sh"
cd "$HERE"
load_env build/hosts.env
BASE=$(base_dir)
COMPOSE="docker compose -f network/compose.yaml --env-file build/hosts.env"

while IFS='|' read -r host profiles; do
    echo "== ${host:-local}:$profiles"
    if [ -n "$host" ]; then
        on "$host" "mkdir -p $BASE"
        rsync -a --delete network build "$host:$BASE/"
        images="hyperledger/fabric-peer:$FABRIC_VERSION"
        [ "$host" = "$ORDERER_SSH" ] && images="$images hyperledger/fabric-orderer:$FABRIC_VERSION"
        [ "$host" = "$P1ORG1_SSH" ] && images="$images hyperledger/fabric-tools:$FABRIC_VERSION"
        on "$host" "gunzip -c $BASE/build/images.tar.gz | docker load -q && for i in $images; do docker pull -q \$i; done"
    fi
    on "$host" "cd $BASE && $COMPOSE $profiles down -v --remove-orphans 2>/dev/null; $COMPOSE $profiles up -d"
done < <(host_profiles)

sleep 5
on "$P1ORG1_SSH" "docker run --rm --network host -v $BASE/build:/work -v $BASE/network/cli.sh:/cli.sh:ro \
    hyperledger/fabric-tools:$FABRIC_VERSION bash /cli.sh"
echo "DEPLOY-DONE"
