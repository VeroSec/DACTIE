#!/usr/bin/env bash
# Analyze a pcap file and report on-the-wire TCP bytes for a specific port.
# Computes inbound (dst port == PORT) and outbound (src port == PORT) byte totals
# using IP/IPv6 header lengths, excluding retransmissions.
#
# Requires: tshark
#
# Usage: ./scripts/analyze_pcap.sh <pcap_file> <tcp_port>

set -euo pipefail

if [ $# -lt 2 ]; then
  echo "Usage: $0 <pcap_file> <tcp_port>" >&2
  exit 1
fi

PCAP="$1"
PORT="$2"

if ! command -v tshark >/dev/null 2>&1; then
  echo "tshark not found. Install it first (e.g., apt-get install tshark)." >&2
  exit 2
fi

# Sum helper: read lines of numbers and output the sum
sum_stream() {
  awk '{s+=$1} END {print s+0}'
}

# Inbound IPv4: ip.len is total length of IP packet including IP header
IN_V4=$(tshark -r "$PCAP" -Y "tcp && ip && tcp.dstport==$PORT && !(tcp.analysis.retransmission || tcp.analysis.fast_retransmission)" -T fields -e ip.len 2>/dev/null | sum_stream)
# Inbound IPv6: ipv6.plen is payload length excluding IPv6 header (40 bytes)
IN_V6_PAY=$(tshark -r "$PCAP" -Y "tcp && ipv6 && tcp.dstport==$PORT && !(tcp.analysis.retransmission || tcp.analysis.fast_retransmission)" -T fields -e ipv6.plen 2>/dev/null | sum_stream)
IN_V6=$(( IN_V6_PAY + 40 * $(tshark -r "$PCAP" -Y "tcp && ipv6 && tcp.dstport==$PORT && !(tcp.analysis.retransmission || tcp.analysis.fast_retransmission)" -T fields -e frame.number 2>/dev/null | wc -l) ))

# Outbound IPv4
OUT_V4=$(tshark -r "$PCAP" -Y "tcp && ip && tcp.srcport==$PORT && !(tcp.analysis.retransmission || tcp.analysis.fast_retransmission)" -T fields -e ip.len 2>/dev/null | sum_stream)
# Outbound IPv6
OUT_V6_PAY=$(tshark -r "$PCAP" -Y "tcp && ipv6 && tcp.srcport==$PORT && !(tcp.analysis.retransmission || tcp.analysis.fast_retransmission)" -T fields -e ipv6.plen 2>/dev/null | sum_stream)
OUT_V6=$(( OUT_V6_PAY + 40 * $(tshark -r "$PCAP" -Y "tcp && ipv6 && tcp.srcport==$PORT && !(tcp.analysis.retransmission || tcp.analysis.fast_retransmission)" -T fields -e frame.number 2>/dev/null | wc -l) ))

INBOUND=$(( IN_V4 + IN_V6 ))
OUTBOUND=$(( OUT_V4 + OUT_V6 ))
TOTAL=$(( INBOUND + OUTBOUND ))

echo "Inbound bytes (IP level):  $INBOUND"
echo "Outbound bytes (IP level): $OUTBOUND"
echo "Total bytes (IP level):    $TOTAL"
