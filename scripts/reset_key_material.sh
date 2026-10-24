#!/bin/bash
# Archive per-node key material so nodes re-register from scratch against a
# fresh authority. Everything is moved aside, never deleted, so a run can be
# recovered if the reset turns out to be the wrong call.
#
# Usage:
#   ./scripts/reset_key_material.sh          # all slots present on this host
#   ./scripts/reset_key_material.sh 0        # only key_material_0 (authority)
#   ./scripts/reset_key_material.sh 3 4 5 6  # only the peer slots
#
# Slot map: 0 authority | 1,2 archives | 3..6 peers 1..4
#
# Stop the affected containers first: a running node rewrites these files on
# exit, which puts the stale state straight back.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

KM_ROOT="./key_material"
[ -d "$KM_ROOT" ] || { echo "no $KM_ROOT here — run from a host that has one"; exit 1; }

slots=("$@")
if [ ${#slots[@]} -eq 0 ]; then
    mapfile -t slots < <(ls -d "$KM_ROOT"/key_material_[0-9]* 2>/dev/null | sed 's/.*key_material_//' | sort -n)
fi
[ ${#slots[@]} -eq 0 ] && { echo "no key_material_* directories found"; exit 1; }

ARCHIVE="$KM_ROOT/.reset-$(date +%Y%m%d-%H%M%S)"
moved_any=0

for i in "${slots[@]}"; do
    dir="$KM_ROOT/key_material_$i"
    [ -d "$dir" ] || { echo "  skip slot $i (no such directory)"; continue; }

    moved=()
    # The authority refuses to start with only one of key_material.json and storage.json.
    for item in key_material.json storage.json slots.json group_saves; do
        [ -e "$dir/$item" ] || continue
        mkdir -p "$ARCHIVE/key_material_$i"
        mv "$dir/$item" "$ARCHIVE/key_material_$i/" && moved+=("$item")
        moved_any=1
    done

    if [ ${#moved[@]} -eq 0 ]; then
        echo "  slot $i: already clean"
    else
        echo "  slot $i: archived ${moved[*]}"
    fi
done

if [ "$moved_any" = 1 ]; then
    echo
    echo "Archived under $ARCHIVE"
    echo "Restore with:  cp -a $ARCHIVE/key_material_<n>/* $KM_ROOT/key_material_<n>/"
else
    echo
    echo "Nothing to archive."
fi

echo
echo "Note: key_material.json carries the node's libp2p keypair, so a reset"
echo "changes its PeerId. If slot 0 was reset, the authority has a new PeerId"
echo "and every other host needs:  ./scripts/set_boot_id.sh <new-peer-id>"
