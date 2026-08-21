# STIX evaluation payloads

STIX 2.1 bundles used as realistic CTI workloads in the latency evaluation.

## Provenance

Derived from the MITRE ATT&CK® ICS domain bundle, `ics-attack.json`, published at
<https://github.com/mitre-attack/attack-stix-data>.

> © 2026 The MITRE Corporation. This work is reproduced and distributed with the
> permission of The MITRE Corporation.

MITRE grants a non-exclusive, royalty-free licence to use ATT&CK® for research,
development, and commercial purposes, provided MITRE's copyright designation and
licence are reproduced in any copy. The full licence is in `LICENSE.txt` and
applies to the contents of this directory only; it is not the licence of the
surrounding project.

## Files

| File | Objects | Size |
|---|---|---|
| `ics-attack.1KiB.json` | 2 | 869 B |
| `ics-attack.4KiB.json` | 6 | 4065 B |
| `ics-attack.16KiB.json` | 20 | 15991 B |
| `ics-attack.64KiB.json` | 77 | 65428 B |
| `ics-attack.256KiB.json` | 242 | 261540 B |
| `ics-attack.json` | 2173 | 2.95 MB (full domain feed) |

The sized bundles pack whole ATT&CK objects until just under each target, so every
file is a valid STIX 2.1 bundle rather than a truncated one — a receiver that
cannot parse the payload measures nothing. Object selection prefers
non-`relationship` objects, which carry the substantive threat content.

Sizes mirror the MISP events in `misp/`, so the two workloads are directly
comparable at each point.

## Reproducing

Regenerate from the upstream bundle with `scripts/make_stix_payloads.py`.
