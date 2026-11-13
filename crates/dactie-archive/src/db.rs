//! The archive's database work, kept off the swarm's event loop.
//!
//! The event loop must never wait for PostgreSQL. While it waits, it reads none of its
//! connections, and libp2p-gossipsub 0.46 keeps an unbounded send queue per connection, so
//! every neighbour buffers every message for the stalled archive (a 6-minute database pause
//! made each neighbour grow by 1.2-2.3 MB/min). Hence:
//! - gossip messages go to a writer task over a queue with a byte budget ([`Writer`]),
//!   which stores them in batches;
//! - requests that need the database run in their own task ([`Db::spawn`]) and hand what
//!   the swarm must do with the result back to the loop as a [`Deferred`].

use std::fmt::Display;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dactie_utils::shared_structs::Response;
use libp2p::request_response::ResponseChannel;
use sqlx::{PgPool, Postgres, QueryBuilder};
use tokio::sync::mpsc;

/// Rows per batch, i.e. per transaction.
const MAX_BATCH_ROWS: usize = 256;
/// Bytes per INSERT statement; a batch of large messages is split into several statements.
const MAX_STATEMENT_BYTES: usize = 8 << 20;
/// Allowance per queued row for its allocations, so that small rows count too.
const ROW_OVERHEAD: usize = 128;
/// At most one warning about dropped messages per this interval.
const WARN_EVERY: Duration = Duration::from_secs(10);
/// Longest wait between two attempts while the database is unreachable.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// A gossip message to store.
#[derive(Debug)]
pub(crate) enum Row {
    Broadcast {
        id: Vec<u8>,
        channel: String,
        topic: String,
        data: Vec<u8>,
    },
    GroupEnc {
        id: Vec<u8>,
        group_id: Vec<u8>,
        data: Vec<u8>,
        nonce: Vec<u8>,
    },
    Proposal {
        id: Vec<u8>,
        group_id: Vec<u8>,
        data: Vec<u8>,
        epoch: u64,
    },
    Commit {
        id: Vec<u8>,
        group_id: Vec<u8>,
        commit: Vec<u8>,
        welcome_option: Option<Vec<u8>>,
        epoch: u64,
    },
}

impl Row {
    /// Bytes the row holds while queued.
    fn size(&self) -> usize {
        ROW_OVERHEAD
            + match self {
                Row::Broadcast {
                    id,
                    channel,
                    topic,
                    data,
                } => id.len() + channel.len() + topic.len() + data.len(),
                Row::GroupEnc {
                    id,
                    group_id,
                    data,
                    nonce,
                } => id.len() + group_id.len() + data.len() + nonce.len(),
                Row::Proposal {
                    id, group_id, data, ..
                } => id.len() + group_id.len() + data.len(),
                Row::Commit {
                    id,
                    group_id,
                    commit,
                    welcome_option,
                    ..
                } => {
                    id.len()
                        + group_id.len()
                        + commit.len()
                        + welcome_option.as_ref().map_or(0, Vec::len)
                }
            }
    }

    /// Commits and proposals are never dropped: they are rare, and a peer that catches up
    /// through this archive needs every one of them to follow its groups' epochs.
    fn is_control(&self) -> bool {
        matches!(self, Row::Proposal { .. } | Row::Commit { .. })
    }

    fn id(&self) -> &[u8] {
        match self {
            Row::Broadcast { id, .. }
            | Row::GroupEnc { id, .. }
            | Row::Proposal { id, .. }
            | Row::Commit { id, .. } => id,
        }
    }
}

/// What the event loop does with the result of database work that ran in its own task.
pub(crate) enum Deferred {
    /// Answer a request that needed the database.
    Respond {
        channel: ResponseChannel<Response>,
        response: Box<Response>,
    },
    /// Compute this archive's partial opening over the stored shares and send it to the
    /// authority, then print `done`, if any.
    OpenPairings {
        signature: Vec<u8>,
        id_shares: Vec<(usize, Vec<u8>)>,
        peer_shares: Vec<(Vec<u8>, Vec<u8>)>,
        done: Option<String>,
    },
}

/// The database as the event loop sees it: queue a gossip message, or start a task.
pub(crate) struct Db {
    pub(crate) pool: PgPool,
    pub(crate) writer: Writer,
    deferred_tx: mpsc::UnboundedSender<Deferred>,
}

impl Db {
    /// Starts the writer task. The event loop polls the returned receiver.
    pub(crate) fn new(
        pool: PgPool,
        queue_budget: usize,
    ) -> (Db, mpsc::UnboundedReceiver<Deferred>) {
        let (deferred_tx, deferred_rx) = mpsc::unbounded_channel();
        let writer = Writer::spawn(pool.clone(), queue_budget);
        (
            Db {
                pool,
                writer,
                deferred_tx,
            },
            deferred_rx,
        )
    }

    /// Runs `job` in its own task, so that the event loop does not wait for the database.
    /// What `job` returns goes back to the loop; an error is logged with `what`.
    pub(crate) fn spawn<F, E>(&self, what: &'static str, job: F)
    where
        F: Future<Output = Result<Option<Deferred>, E>> + Send + 'static,
        E: Display + Send + 'static,
    {
        let deferred_tx = self.deferred_tx.clone();
        tokio::spawn(async move {
            match job.await {
                Ok(Some(deferred)) => {
                    // Fails only if the event loop has ended, and then nobody is left to answer.
                    let _ = deferred_tx.send(deferred);
                }
                Ok(None) => {}
                Err(e) => log::error!("{what}: {e}"),
            }
        });
    }
}

/// The event loop's end of the writer's queue.
pub(crate) struct Writer {
    tx: mpsc::UnboundedSender<Row>,
    /// Bytes queued and not yet stored; the writer subtracts what it stored.
    queued: Arc<AtomicUsize>,
    budget: usize,
    dropped: Arc<AtomicU64>,
    dropped_at_warning: u64,
    last_warning: Option<Instant>,
}

impl Writer {
    fn spawn(pool: PgPool, budget: usize) -> Writer {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        tokio::spawn(run(pool, rx, queued.clone(), dropped.clone(), budget));
        Writer {
            tx,
            queued,
            budget,
            dropped,
            dropped_at_warning: 0,
            last_warning: None,
        }
    }

    /// Queues `row` for storage without waiting. Over the budget, a broadcast or group
    /// message is dropped instead: it has already been forwarded, only this archive's copy
    /// is lost. Blocking here would stall the event loop, which is what the queue prevents.
    /// Returns whether the row was queued.
    pub(crate) fn store(&mut self, row: Row) -> bool {
        let size = row.size();
        let queued = self.queued.load(Ordering::Relaxed);
        if !row.is_control() && queued + size > self.budget {
            let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if self
                .last_warning
                .is_none_or(|at| at.elapsed() >= WARN_EVERY)
            {
                log::warn!(
                    "Database queue full ({:.1} of {} MiB): not storing {} messages since the last \
                     warning, {} in total. PostgreSQL is slower than the gossip arriving; the \
                     messages are still forwarded. See --db-queue-mib",
                    queued as f64 / (1 << 20) as f64,
                    self.budget >> 20,
                    dropped - self.dropped_at_warning,
                    dropped
                );
                self.last_warning = Some(Instant::now());
                self.dropped_at_warning = dropped;
            }
            return false;
        }
        self.queued.fetch_add(size, Ordering::Relaxed);
        if self.tx.send(row).is_err() {
            self.queued.fetch_sub(size, Ordering::Relaxed);
            log::error!("Database writer has ended, not storing a message");
            return false;
        }
        true
    }
}

/// The writer task: stores whatever is queued, up to MAX_BATCH_ROWS rows per transaction.
async fn run(
    pool: PgPool,
    mut rx: mpsc::UnboundedReceiver<Row>,
    queued: Arc<AtomicUsize>,
    dropped: Arc<AtomicU64>,
    budget: usize,
) {
    let mut batch = Vec::with_capacity(MAX_BATCH_ROWS);
    let mut reported = 0;
    while rx.recv_many(&mut batch, MAX_BATCH_ROWS).await > 0 {
        let bytes: usize = batch.iter().map(Row::size).sum();
        store_batch(&pool, &batch).await;
        queued.fetch_sub(bytes, Ordering::Relaxed);
        batch.clear();
        // Once the backlog is gone, account for every message that was not stored.
        let total = dropped.load(Ordering::Relaxed);
        if total > reported && queued.load(Ordering::Relaxed) <= budget / 4 {
            log::warn!(
                "Database caught up. {} messages were not stored while it was slow, {} since the start",
                total - reported,
                total
            );
            reported = total;
        }
    }
    log::warn!("Database writer ended: the event loop has gone");
}

/// Stores `rows` in one transaction. While the database is unreachable, retries with
/// backoff, so nothing is lost to a restart of PostgreSQL. If the database rejects the batch,
/// stores row by row, so that one bad row costs only itself.
async fn store_batch(pool: &PgPool, rows: &[Row]) {
    match retry(pool, rows).await {
        Ok(()) => {}
        Err(e) => {
            log::warn!(
                "Database rejected a batch of {} messages ({e}), storing them one by one",
                rows.len()
            );
            for row in rows {
                if let Err(e) = retry(pool, std::slice::from_ref(row)).await {
                    log::error!("Not storing message {}: {e}", hex::encode(row.id()));
                }
            }
        }
    }
}

/// Writes `rows`, retrying for as long as the error is about reaching the database.
async fn retry(pool: &PgPool, rows: &[Row]) -> Result<(), sqlx::Error> {
    let mut backoff = Duration::from_millis(250);
    loop {
        match write(pool, rows).await {
            Err(e) if is_unreachable(&e) => {
                log::warn!(
                    "Database unreachable ({e}), retrying {} messages in {backoff:?}",
                    rows.len()
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            result => return result,
        }
    }
}

/// Errors about reaching the database, as opposed to the database refusing the rows.
fn is_unreachable(e: &sqlx::Error) -> bool {
    matches!(
        e,
        sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::Protocol(_)
            | sqlx::Error::PoolTimedOut
            | sqlx::Error::WorkerCrashed
    )
}

/// One transaction with one multi-row INSERT per table (more if the rows are large).
/// A message already stored is skipped: message IDs are content hashes.
async fn write(pool: &PgPool, rows: &[Row]) -> Result<(), sqlx::Error> {
    let mut tables: [Vec<&Row>; 4] = Default::default();
    for row in rows {
        let table = match row {
            Row::Broadcast { .. } => 0,
            Row::GroupEnc { .. } => 1,
            Row::Proposal { .. } => 2,
            Row::Commit { .. } => 3,
        };
        tables[table].push(row);
    }
    let mut tx = pool.begin().await?;
    for rows in tables.iter().filter(|rows| !rows.is_empty()) {
        for statement in by_size(rows) {
            insert(statement).build().execute(&mut *tx).await?;
        }
    }
    tx.commit().await
}

/// Splits `rows` into statements of at most MAX_STATEMENT_BYTES (at least one row each).
fn by_size<'a>(rows: &[&'a Row]) -> Vec<Vec<&'a Row>> {
    let mut statements = Vec::new();
    let mut current = Vec::new();
    let mut bytes = 0;
    for &row in rows {
        if !current.is_empty() && bytes + row.size() > MAX_STATEMENT_BYTES {
            statements.push(std::mem::take(&mut current));
            bytes = 0;
        }
        bytes += row.size();
        current.push(row);
    }
    if !current.is_empty() {
        statements.push(current);
    }
    statements
}

/// The INSERT for rows of one table.
fn insert<'a>(rows: Vec<&'a Row>) -> QueryBuilder<'a, Postgres> {
    let head = match rows[0] {
        Row::Broadcast { .. } => "INSERT INTO broadcast (id, channel, topic, data) ",
        Row::GroupEnc { .. } => "INSERT INTO group_enc (id, group_id, data, nonce) ",
        Row::Proposal { .. } => "INSERT INTO proposal (id, group_id, data, epoch) ",
        Row::Commit { .. } => "INSERT INTO commit (id, group_id, commit, welcome_option, epoch) ",
    };
    let mut query = QueryBuilder::new(head);
    query.push_values(rows, |mut values, row| match row {
        Row::Broadcast {
            id,
            channel,
            topic,
            data,
        } => {
            values
                .push_bind(id.as_slice())
                .push_bind(channel.as_str())
                .push_bind(topic.as_str())
                .push_bind(data.as_slice());
        }
        Row::GroupEnc {
            id,
            group_id,
            data,
            nonce,
        } => {
            values
                .push_bind(id.as_slice())
                .push_bind(group_id.as_slice())
                .push_bind(data.as_slice())
                .push_bind(nonce.as_slice());
        }
        // PostgreSQL has no unsigned 64-bit type; epochs are stored as BIGINT.
        Row::Proposal {
            id,
            group_id,
            data,
            epoch,
        } => {
            values
                .push_bind(id.as_slice())
                .push_bind(group_id.as_slice())
                .push_bind(data.as_slice())
                .push_bind(*epoch as i64);
        }
        Row::Commit {
            id,
            group_id,
            commit,
            welcome_option,
            epoch,
        } => {
            values
                .push_bind(id.as_slice())
                .push_bind(group_id.as_slice())
                .push_bind(commit.as_slice())
                .push_bind(welcome_option.as_deref())
                .push_bind(*epoch as i64);
        }
    });
    query.push(" ON CONFLICT (id) DO NOTHING");
    query
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broadcast(len: usize) -> Row {
        Row::Broadcast {
            id: vec![1; 20],
            channel: "channel_1".into(),
            topic: "t".into(),
            data: vec![0; len],
        }
    }

    #[test]
    fn statements_stay_under_the_byte_limit() {
        let rows: Vec<Row> = (0..40).map(|_| broadcast(1 << 20)).collect();
        let refs: Vec<&Row> = rows.iter().collect();
        let statements = by_size(&refs);
        assert_eq!(statements.iter().map(Vec::len).sum::<usize>(), 40);
        for statement in &statements {
            assert!(
                statement.len() == 1
                    || statement.iter().map(|r| r.size()).sum::<usize>() <= MAX_STATEMENT_BYTES
            );
        }
        assert_eq!(statements.len(), 6); // 7 rows of 1 MiB + overhead fit in 8 MiB
    }

    #[test]
    fn a_row_larger_than_the_limit_gets_its_own_statement() {
        let rows = [
            broadcast(1),
            broadcast(MAX_STATEMENT_BYTES + 1),
            broadcast(1),
        ];
        let refs: Vec<&Row> = rows.iter().collect();
        let sizes: Vec<usize> = by_size(&refs).iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![1, 1, 1]);
    }

    #[tokio::test]
    async fn over_budget_drops_data_but_never_commits() {
        // A writer without a task behind it: nothing drains the queue.
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut writer = Writer {
            tx,
            queued: Arc::new(AtomicUsize::new(0)),
            budget: 10_000,
            dropped: Arc::new(AtomicU64::new(0)),
            dropped_at_warning: 0,
            last_warning: None,
        };
        let stored = (0..20).filter(|_| writer.store(broadcast(1_000))).count();
        assert_eq!(stored, 8); // 8 x (1,000 + 20 + 9 + 1 + 128) bytes fit in 10,000
        assert_eq!(writer.dropped.load(Ordering::Relaxed), 12);
        let commit = Row::Commit {
            id: vec![2; 20],
            group_id: vec![3; 16],
            commit: vec![0; 5_000],
            welcome_option: None,
            epoch: 7,
        };
        assert!(
            writer.store(commit),
            "a commit is queued even over the budget"
        );
        assert!(!writer.store(broadcast(1)));
    }
}
