//! Criterion microbenchmarks for the threshold group signature used by DACTIE.
//!
//! Defaults mirror AttriChain's setting (3 tracing members, threshold 3) so the
//! numbers can be put next to its Table 1. Override with environment variables:
//!   DACTIE_BENCH_GROUP_SIZES  comma-separated member counts for opening (default 10,100,500)
//!   DACTIE_BENCH_ARCHIVES     number of archives / openers (default 3)
//!   DACTIE_BENCH_THRESHOLD    opening threshold t (default 3)
//!   DACTIE_BENCH_MSG_BYTES    signed payload size in bytes (default 1024)
//!   DACTIE_BENCH_PEERS_PER_MEMBER  sharing peers per member for opening (default 1: the
//!                             peer a member joined from)

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use dactie_benchmark::{join, Group};
use std::collections::HashMap;
use std::time::Duration;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn group_sizes() -> Vec<usize> {
    std::env::var("DACTIE_BENCH_GROUP_SIZES")
        .ok()
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![10, 100, 500])
}

fn params() -> (usize, usize, usize) {
    (
        env_usize("DACTIE_BENCH_ARCHIVES", 3),
        env_usize("DACTIE_BENCH_THRESHOLD", 3),
        env_usize("DACTIE_BENCH_MSG_BYTES", 1024),
    )
}

/// Per-message cost on the publishing and receiving side; independent of group size.
fn sign_verify(c: &mut Criterion) {
    let (archives, threshold, msg_bytes) = params();
    let group = Group::new(2, archives, threshold);
    let msg = vec![0xA5u8; msg_bytes];
    let sig = group.sign(0, &msg);
    eprintln!(
        "signature size: {} bytes (bincode)",
        bincode::serialize(&sig).expect("serialize").len()
    );

    let mut g = c.benchmark_group("per_message");
    g.bench_function("sign", |b| b.iter(|| group.sign(0, &msg)));
    g.bench_function("verify", |b| {
        b.iter(|| thressig::verify(&sig, &group.grpkey, &msg.as_slice()).expect("verify"))
    });
    g.finish();
}

/// One member joining: member side, authority side and share verification together.
fn member_join(c: &mut Criterion) {
    let (archives, threshold, _) = params();
    let group = Group::new(0, archives, threshold);
    let mut g = c.benchmark_group("join");
    g.sample_size(20);
    g.bench_function(BenchmarkId::new("full_join", format!("{archives}_archives")), |b| {
        b.iter_batched(
            HashMap::new,
            |mut gml| {
                join(0, &group.grpkey, &group.mgrkey, threshold, &group.opener_pub_keys, &mut gml)
            },
            BatchSize::SmallInput,
        )
    });
    g.finish();
}

/// Opening a signature. Each archive computes one pairing per stored share, i.e. per
/// member and per sharing peer, because the signer is unknown; the combining step then
/// checks every member and every sharing peer, so both scale linearly with the shares.
fn opening(c: &mut Criterion) {
    let (archives, threshold, msg_bytes) = params();
    let peers_per_member = env_usize("DACTIE_BENCH_PEERS_PER_MEMBER", 1);
    let msg = vec![0xA5u8; msg_bytes];
    let mut g = c.benchmark_group("open");
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(20));
    for n in group_sizes() {
        let group = Group::with_peers(n, peers_per_member, archives, threshold);
        eprintln!(
            "open: {n} members, {peers_per_member} sharing peers each, {} stored shares",
            group.stored_shares()
        );
        let sig = group.sign(n / 2, &msg);
        let all_sj = group.collect_pairings(&sig);
        let (members, peers) = group.open(&sig, &all_sj);
        assert_eq!(members, vec![n / 2]);
        assert_eq!(peers.len(), peers_per_member);

        g.bench_with_input(BenchmarkId::new("archive_partial_open", n), &n, |b, _| {
            b.iter(|| group.archive_open_pairings(&sig, 0))
        });
        g.bench_with_input(BenchmarkId::new("combine", n), &n, |b, _| {
            b.iter(|| group.open(&sig, &all_sj))
        });
    }
    g.finish();
}

criterion_group!(benches, sign_verify, member_join, opening);
criterion_main!(benches);
