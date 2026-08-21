#!/usr/bin/env python3
"""Regenerate the STIX evaluation payloads in stix/ from an upstream ATT&CK bundle.

    curl -LO https://raw.githubusercontent.com/mitre-attack/attack-stix-data/master/ics-attack/ics-attack.json
    ./scripts/make_stix_payloads.py ics-attack.json

Sizes mirror the MISP events in misp/ so the two workloads compare point for point.
See stix/NOTICE.md for provenance and the MITRE licence terms.
"""
import json
import os
import sys

TARGETS = {"1KiB": 1024, "4KiB": 4096, "16KiB": 16384, "64KiB": 65536, "256KiB": 262144}
BUNDLE_ID = "bundle--0b0a1f2c-9f3e-4c5a-8d7b-1e2f3a4b5c6d"


def bundle(objects):
    return {"type": "bundle", "id": BUNDLE_ID, "objects": objects}


def main(src_path, out_dir="stix"):
    src = json.load(open(src_path))
    objs = src["objects"]
    os.makedirs(out_dir, exist_ok=True)

    # Relationships last: they are the bulk of the bundle but carry little
    # substantive threat content, so a small bundle made only of them would not
    # represent what an organisation actually shares.
    ordered = sorted(objs, key=lambda o: (o["type"] == "relationship", len(json.dumps(o))))

    for name, limit in TARGETS.items():
        chosen = []
        for o in ordered:
            # Pack whole objects only: a bundle truncated mid-object does not
            # parse, and an unparseable payload measures nothing.
            if len(json.dumps(bundle(chosen + [o]))) > limit:
                continue
            chosen.append(o)
        path = os.path.join(out_dir, f"ics-attack.{name}.json")
        with open(path, "w") as fh:
            json.dump(bundle(chosen), fh)
        print(f"{path}: {os.path.getsize(path)} bytes, {len(chosen)} objects")

    path = os.path.join(out_dir, "ics-attack.json")
    with open(path, "w") as fh:
        json.dump(src, fh)
    print(f"{path}: {os.path.getsize(path)} bytes, {len(objs)} objects")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(*sys.argv[1:])
