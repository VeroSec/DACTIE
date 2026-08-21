#!/usr/bin/env bash
# Capture on-the-wire bytes for a DACTIE peer process.
# Usage: ./scripts/capture_on_wire.sh <tcp_port> <output_pcap>
# Example: ./scripts/capture_on_wire.sh 10001 /tmp/dactie_10001.pcap

set -euo pipefail

if [ $# -lt 2 ]; then
  echo "Usage: $0 <tcp_port> <output_pcap>" >&2
  exit 1
fi

PORT="$1"
OUT="$2"

if ! command -v tcpdump >/dev/null 2>&1; then
  echo "tcpdump not found. Install it first (e.g., apt-get install tcpdump)." >&2
  exit 2
fi

echo "Capturing on-the-wire TCP traffic on port ${PORT} to ${OUT}..."
sudo tcpdump -i any tcp port "${PORT}" -w "${OUT}" -s 0
