import csv
import os
import re
import sys

def parse_file(input_path, output_path):
    starting_re = re.compile(r"^Starting Run .*?peer_n=(\d+),")
    received_re = re.compile(r"^Received\s+(\d+)\s+messages from\s+(\d+)\s+peers\.")
    direct_re = re.compile(r"^Direct from Publisher:\s+(\d+)")
    relayed_re = re.compile(r"^Relayed:\s+(\d+)")
    percent_re = re.compile(r"^Percentage of direct peers:\s+([^\s]+)")
    prop_re = re.compile(r"^Number of Propagating Peers:\s+(\d+)")

    rows = []
    current = None

    with open(input_path, "r", encoding="utf-8") as f:
        for raw_line in f:
            line = raw_line.strip()
            if not line:
                continue

            m = starting_re.match(line)
            if m:
                # Start a new run block
                current = {
                    "total_peers": int(m.group(1)),
                    "received_messages": None,
                    "all_expected_messages": None,
                    "from_publisher": None,
                    "relayed": None,
                    "direct_publisher_percent": None,
                    "number_propagating": None,
                }
                continue

            if current is None:
                # We're not inside a block we care about
                continue

            m = received_re.match(line)
            if m:
                current["received_messages"] = int(m.group(1))
                current["all_expected_messages"] = int(m.group(2))
                continue

            m = direct_re.match(line)
            if m:
                current["from_publisher"] = int(m.group(1))
                continue

            m = relayed_re.match(line)
            if m:
                current["relayed"] = int(m.group(1))
                continue

            m = percent_re.match(line)
            if m:
                val = m.group(1)
                # Try to parse as float; if NaN or something odd, keep as string
                try:
                    current["direct_publisher_percent"] = float(val)
                except ValueError:
                    current["direct_publisher_percent"] = val
                continue

            m = prop_re.match(line)
            if m:
                current["number_propagating"] = int(m.group(1))
                # Block complete; only keep fully populated rows
                if (
                    current["total_peers"] is not None
                    and current["received_messages"] is not None
                    and current["all_expected_messages"] is not None
                    and current["from_publisher"] is not None
                    and current["relayed"] is not None
                    and current["direct_publisher_percent"] is not None
                    and current["number_propagating"] is not None
                ):
                    rows.append(current)
                # Reset to look for next "Starting Run..."
                current = None
                continue

    # Write CSV
    fieldnames = [
        "total_peers",
        "received_messages",
        "all_expected_messages",
        "from_publisher",
        "relayed",
        "direct_publisher_percent",
        "number_propagating",
    ]

    with open(output_path, "w", newline="", encoding="utf-8") as out_f:
        writer = csv.writer(out_f)
        writer.writerow(fieldnames)
        for row in rows:
            writer.writerow([
                row["total_peers"],
                row["received_messages"],
                row["all_expected_messages"],
                row["from_publisher"],
                row["relayed"],
                row["direct_publisher_percent"],
                row["number_propagating"],
            ])

def main():
    if len(sys.argv) != 3:
        print("Usage: python3 parse_output_to_csv.py <input_path> <output_path>")
        sys.exit(1)

    input_path = sys.argv[1]
    output_path = sys.argv[2]

    if not os.path.isfile(input_path):
        print(f"Input file not found: {input_path}")
        sys.exit(1)

    parse_file(input_path, output_path)
    print(f"Wrote CSV to {output_path}")

if __name__ == "__main__":
    main()
