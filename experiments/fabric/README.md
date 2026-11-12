# Hyperledger Fabric on the DACTIE testbed

Most related work on anonymous CTI sharing builds on Hyperledger Fabric. To compare DACTIE with it on equal terms, this directory runs Fabric on the same four hosts as DACTIE's arrival-rate sweep (`scripts/rate_sweep.sh`), with the same arrival rates, the same 1 KiB MISP record and the same measurement window. Earlier versions of the comparison read Fabric's numbers from Thakkar et al.'s plot, measured on other hardware with Fabric 1.0.

## Setup

| | Fabric (this directory) | DACTIE (`scripts/rate_sweep.sh`) |
|---|---|---|
| Version | Fabric 2.5.16 (LTS), Raft ordering | public `main` |
| minerva1 | orderer | AA |
| minerva2 | peer0.org1 (receiver) | archive 1, peer_1 (receiver) |
| minerva3 | peer0.org2 | archive 2, peer_2 |
| minerva4 | peer1.org1 (gateway), peer1.org2, load generator | peer_3, peer_4 (publisher) |
| Hosts | 4 vCPUs (Xeon Gold 6342), 15 GB each, one LAN | same |
| Record | 1 KiB MISP event (`workloads/misp/…1KiB.json`), or 20 bytes as in Thakkar et al. | 1 KiB MISP event |
| Rates | 25, 50, 75, 100, 110, 125, 150, 175 per second (the MISP record also 200 to 1,000), about 100 s each | 25 to 175 per second |
| Latency | proposal to commit at peer0.org1 | publication to delivery at peer_1 |
| Window | 5 to 55 s after the first commit (`scripts/rate_sweep_summary.py`) | same |

Fabric is set up for its best case on this hardware:
- **Ordering:** one Raft orderer.
- **Endorsement:** one peer (policy `OR(Org1.peer, Org2.peer)`); the gateway is the local peer1.org1.
- **State database:** LevelDB.
- **Blocks:** every peer pulls them from the orderer itself, without gossip in between. A block holds 30 transactions or is cut after 1 s (Thakkar et al.'s default).
- **Chaincode:** `put(key, value)` writes one record under a new key, so no transaction conflicts with another (`chaincode/`).
- **Security:** TLS everywhere.

The load generator (`loadgen/`) submits on a fixed schedule (open loop) and timestamps each transaction twice in the same process: when the proposal is created and when the block holding it reaches peer0.org1. Its receipts file has the columns of DACTIE's `raw.csv.gz`, so one summary script serves both systems. Transactions that fail, are invalid or are not committed within 60 s after the last submission count as loss.

## Running it

Needs Docker (rootless works) and, for the minervas, ssh access as in `docs/RUNBOOK.md`.

```
cd experiments/fabric
./generate.sh hosts/minervas.env   # crypto material, channel block, chaincode package, images -> build/
./deploy.sh                        # copies build/ to the hosts, starts the network, creates channel and chaincode
./sweep.sh misp    ../../results/fabric/rate-sweep-<date>/misp-1KiB
./sweep.sh thakkar ../../results/fabric/rate-sweep-<date>/thakkar-20B
```

`hosts/local.env` runs everything on one machine for a try-out; `RATES="25 100" DURATION=20 ./sweep.sh misp /tmp/try` takes about three minutes. A redeploy deletes the ledgers. To stop the network on a host: `docker compose -f network/compose.yaml --env-file build/hosts.env --profile <m1|m2|m3|m4|local> down -v` in `REMOTE_DIR` (`/opt/vero/fabric-comparison` on the minervas).

Each sweep directory holds:
- `raw.csv.gz`: every commit at peer0.org1, with its block number.
- `summary.csv`: throughput, mean and p95 latency, loss per rate.
- `r<rate>.failures.csv`: failed or uncommitted transactions.
- `r<rate>.log`: the load generator's output.
- `cpu/r<rate>.cpu`: docker stats of every Fabric container 40 s into the run.

## Differences that remain

- **Ordering:** Fabric orders all transactions into a single ledger and validates them on every peer, and its latency includes that. DACTIE provides no total order, which CTI sharing does not need.
- **Signatures:** DACTIE peers verify a pairing-based group signature per message. Fabric peers check ECDSA signatures of clients and endorsers.
- **Versus Thakkar et al.:** they used Fabric 1.0 with Kafka ordering on 32-vCPU nodes, so absolute numbers differ from theirs. The 20-byte workload (`thakkar`) shows how far.
