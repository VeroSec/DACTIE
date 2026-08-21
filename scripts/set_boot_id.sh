#!/bin/bash
# Point this host's archives and peers at the authority's PeerId.
#
# Usage:
#   ./scripts/set_boot_id.sh 12D3KooW...
#
# The six services that bootstrap (archive_node1, archive_node2, peer_1..4)
# read BOOT_PEER_ID from key_material/boot_id.env via env_file. Compose bakes
# env_file values into a container at CREATE time, so after changing this you
# must recreate rather than restart:
#
#   docker compose --profile <node> up -d --force-recreate <services>
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PEER_ID="${1:-}"
if [ -z "$PEER_ID" ]; then
    echo "usage: $0 <authority-peer-id>"
    [ -f ./key_material/boot_id.env ] && echo "current: $(cat ./key_material/boot_id.env)"
    exit 1
fi

# libp2p ed25519 PeerIds are base58btc, 52 chars starting 12D3KooW
if ! printf '%s' "$PEER_ID" | grep -qE '^12D3KooW[1-9A-HJ-NP-Za-km-z]{44}$'; then
    echo "warning: '$PEER_ID' does not look like a libp2p PeerId (expected 12D3KooW + 44 base58 chars)"
    read -r -p "write it anyway? [y/N] " reply
    [ "$reply" = "y" ] || [ "$reply" = "Y" ] || exit 1
fi

mkdir -p ./key_material
echo "BOOT_PEER_ID=$PEER_ID" > ./key_material/boot_id.env
echo "wrote ./key_material/boot_id.env:"
cat ./key_material/boot_id.env
echo
echo "Now recreate the services on this host, e.g.:"
echo "  docker compose --profile node117 up -d --force-recreate archive_node1 peer_1"
echo "(a plain restart keeps the old value)"
