#!/usr/bin/env python3

import csv
import glob
import os
from typing import List


BASE_DIR = os.path.dirname(os.path.abspath(__file__))
OUTPUT_DIR = os.path.join(BASE_DIR, "output")
INPUT_PATTERN = os.path.join(OUTPUT_DIR, "*", "ex*.csv")
OUTPUT_CSV = os.path.join(BASE_DIR, "eclipse_parsed.csv")


def collect_rows() -> List[List[object]]:
    """Collect rows from all matching experiment CSV files with derived columns."""
    input_files = sorted(glob.glob(INPUT_PATTERN))
    rows: List[List[object]] = []

    for path in input_files:
        with open(path, newline="", encoding="utf-8") as f:
            reader = csv.reader(f)
            try:
                header = next(reader)
            except StopIteration:
                # Empty file
                continue

            # Expect the basic columns in each source CSV
            try:
                idx_total = header.index("total_peers")
                idx_honest = header.index("honest_peers")
                idx_attacker = header.index("attacker_peers")
                idx_received = header.index("received_messages")
            except ValueError:
                # Unexpected header format, skip this file
                continue

            for row in reader:
                if not row or all(not cell.strip() for cell in row):
                    continue

                try:
                    total_peers = int(row[idx_total])
                    honest_peers = int(row[idx_honest])
                    attacker_peers = int(row[idx_attacker])
                    received_messages = int(row[idx_received])
                except (ValueError, IndexError):
                    # Skip malformed lines
                    continue

                if total_peers <= 0:
                    # Avoid division by zero
                    continue

                attacker_ratio = attacker_peers / total_peers

                # Derived columns based on your instructions
                received_messages_soll = honest_peers - 1
                if received_messages_soll > 0:
                    message_ratio: object = received_messages / received_messages_soll
                else:
                    # No valid denominator; leave ratio empty
                    message_ratio = ""

                rows.append(
                    [
                        total_peers,
                        honest_peers,
                        attacker_peers,
                        attacker_ratio,
                        received_messages,
                        message_ratio,
                        received_messages_soll,
                    ]
                )

    return rows


def main() -> None:
    rows = collect_rows()

    header = [
        "total_peers",
        "honest_peers",
        "attacker_peers",
        "attacker_ratio",
        "received_messages",
        "message_ratio",
        "received_messages_soll",
    ]

    with open(OUTPUT_CSV, "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(header)
        writer.writerows(rows)

    print(f"Wrote {len(rows)} rows to {OUTPUT_CSV}")


if __name__ == "__main__":
    main()

mport csv
import glob
import os
from typing import List


BASE_DIR = os.path.dirname(os.path.abspath(__file__))
OUTPUT_DIR = os.path.join(BASE_DIR, "output")
INPUT_PATTERN = os.path.join(OUTPUT_DIR, "*", "ex*.csv")
OUTPUT_CSV = os.path.join(BASE_DIR, "eclipse_parsed.csv")


def collect_rows() -> List[List[object]]:
        """Collect rows from all matching experiment CSV files with derived columns."""
            input_files = sorted(glob.glob(INPUT_PATTERN))
                rows: List[List[object]] = []

                    for path in input_files:
                                with open(path, newline="", encoding="utf-8") as f:
                                                reader = csv.reader(f)
                                                            try:
                                                                                header = next(reader)
                                                                                            except StopIteration:
                                                                                                                # Empty file
                                                                                                                                continue

                                                                                                                                        # Expect the basic columns in each source CSV
                                                                                                                                                    try:
                                                                                                                                                                        idx_total = header.index("total_peers")
                                                                                                                                                                                        idx_honest = header.index("honest_peers")
                                                                                                                                                                                                        idx_attacker = header.index("attacker_peers")
                                                                                                                                                                                                                        idx_received = header.index("received_messages")
                                                                                                                                                                                                                                    except ValueError:
                                                                                                                                                                                                                                                        # Unexpected header format, skip this file
                                                                                                                                                                                                                                                                        continue

                                                                                                                                                                                                                                                                                for row in reader:
                                                                                                                                                                                                                                                                                                    if not row or all(not cell.strip() for cell in row):
                                                                                                                                                                                                                                                                                                                            continue

                                                                                                                                                                                                                                                                                                                                        try:
                                                                                                                                                                                                                                                                                                                                                                total_peers = int(row[idx_total])
                                                                                                                                                                                                                                                                                                                                                                                    honest_peers = int(row[idx_honest])
                                                                                                                                                                                                                                                                                                                                                                                                        attacker_peers = int(row[idx_attacker])
                                                                                                                                                                                                                                                                                                                                                                                                                            received_messages = int(row[idx_received])
                                                                                                                                                                                                                                                                                                                                                                                                                                            except (ValueError, IndexError):
                                                                                                                                                                                                                                                                                                                                                                                                                                                                    # Skip malformed lines
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        continue

                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    if total_peers <= 0:
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            # Avoid division by zero
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                continue

                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            attacker_ratio = attacker_peers / total_peers

                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            # Derived columns based on your instructions
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            received_messages_soll = honest_peers - 1
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            if received_messages_soll > 0:
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    message_ratio: object = received_messages / received_messages_soll
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    else:
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            # No valid denominator; leave ratio empty
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                message_ratio = ""

                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                rows.append(
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            [
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        total_peers,
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                honest_peers,
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        attacker_peers,
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                attacker_ratio,
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        received_messages,
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                message_ratio,
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        received_messages_soll,
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            ]
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            )

                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    return rows


                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                def main() -> None:
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        rows = collect_rows()

                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            header = [
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            "total_peers",
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    "honest_peers",
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            "attacker_peers",
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    "attacker_ratio",
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            "received_messages",
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    "message_ratio",
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            "received_messages_soll",
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                ]

                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                with open(OUTPUT_CSV, "w", newline="", encoding="utf-8") as f:
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            writer = csv.writer(f)
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    writer.writerow(header)
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            writer.writerows(rows)

                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                print(f"Wrote {len(rows)} rows to {OUTPUT_CSV}")


                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                if __name__ == "__main__":
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        main()


