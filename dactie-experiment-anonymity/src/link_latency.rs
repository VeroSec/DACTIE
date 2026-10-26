//! Simulated link latency for the single-process experiment.
//!
//! All peers of a run share one process, so without this every link has zero
//! delay and CPU scheduling alone decides which copy of a message a peer gets
//! first. A copy relayed over two idle links then often beats the direct one,
//! and peers record fewer direct receipts from the source than a real network
//! would show, where every hop adds delay. Here each peer sits at a random
//! point of a unit square, and a link's one-way delay grows with the distance
//! between its ends, from `min` for neighbours to `max` for opposite corners.
//! Delays thus obey the triangle inequality, as they roughly do on the
//! Internet: a detour via a third peer is never faster than the direct link.
//! With delays drawn independently per link, detours often won: at 100 peers,
//! about two of the source's ten mesh peers still got a relayed copy first.
//! Bytes read from the socket are passed on only once they are that old, in
//! order as on a TCP link; bandwidth is not modelled.
//!
//! A peer's position derives from its listening port. Outgoing connections
//! reuse that port, so both ends of a link see both listening ports and
//! compute the same delay.

use futures::io::{AsyncRead, AsyncWrite};
use std::collections::VecDeque;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use tokio::time::{Duration, Instant, Sleep};

/// One-way delay range in milliseconds; `0` turns the simulation off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LatencyRange {
    pub(crate) min_ms: u64,
    pub(crate) max_ms: u64,
}

impl LatencyRange {
    pub(crate) const OFF: LatencyRange = LatencyRange { min_ms: 0, max_ms: 0 };

    pub(crate) fn is_off(&self) -> bool {
        self.max_ms == 0
    }

    /// The delay of the link between the peers listening on ports `a` and `b`.
    /// It is symmetric, and ports differ between runs, so positions do too.
    pub(crate) fn for_ports(&self, a: u16, b: u16) -> Duration {
        let ((ax, ay), (bx, by)) = (position(a), position(b));
        let distance = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt() / std::f64::consts::SQRT_2;
        let span = (self.max_ms - self.min_ms) as f64;
        Duration::from_micros(((self.min_ms as f64 + span * distance) * 1000.0).round() as u64)
    }
}

/// A peer's point in the unit square, from its listening port.
fn position(port: u16) -> (f64, f64) {
    let mut h = DefaultHasher::new();
    port.hash(&mut h);
    let bits = h.finish();
    let unit = |v: u64| (v & 0xffff_ffff) as f64 / u32::MAX as f64;
    (unit(bits), unit(bits >> 32))
}

impl FromStr for LatencyRange {
    type Err = String;

    /// `0` (off), `<ms>` (fixed) or `<min>,<max>`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parse = |v: &str| {
            v.trim()
                .parse::<u64>()
                .map_err(|_| format!("Invalid link latency in ms: '{v}'"))
        };
        let (min_ms, max_ms) = match s.split_once(',') {
            Some((min, max)) => (parse(min)?, parse(max)?),
            None => (parse(s)?, parse(s)?),
        };
        if min_ms > max_ms {
            return Err(format!("Link latency minimum above maximum: '{s}'"));
        }
        Ok(LatencyRange { min_ms, max_ms })
    }
}

impl std::fmt::Display for LatencyRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{}", self.min_ms, self.max_ms)
    }
}

/// Bytes a connection may hold in delay. Beyond it the socket is left alone,
/// so TCP's flow control slows the sender as it would without the delay;
/// reading everything at once let a CPU-bound run pile up seconds of backlog.
const QUEUE_LIMIT: usize = 64 * 1024;

/// A stream whose reads lag `delay` behind the socket.
pub(crate) struct Delayed<S> {
    inner: S,
    delay: Duration,
    /// Received bytes with the time they may be passed on.
    queue: VecDeque<(Instant, Vec<u8>)>,
    /// How much of the front chunk was already passed on.
    offset: usize,
    /// Bytes in `queue` not yet passed on.
    queued: usize,
    /// The socket's end of stream or error, passed on after the same delay.
    ended: Option<(Instant, Option<io::Error>)>,
    sleep: Pin<Box<Sleep>>,
    scratch: Box<[u8; 16 * 1024]>,
}

impl<S> Delayed<S> {
    pub(crate) fn new(inner: S, delay: Duration) -> Self {
        Delayed {
            inner,
            delay,
            queue: VecDeque::new(),
            offset: 0,
            queued: 0,
            ended: None,
            sleep: Box::pin(tokio::time::sleep(Duration::ZERO)),
            scratch: Box::new([0; 16 * 1024]),
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Delayed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        // Take in what the socket has, stamped with its release time.
        while this.ended.is_none() && this.queued < QUEUE_LIMIT {
            match Pin::new(&mut this.inner).poll_read(cx, &mut this.scratch[..]) {
                Poll::Ready(Ok(0)) => this.ended = Some((Instant::now() + this.delay, None)),
                Poll::Ready(Ok(n)) => {
                    this.queued += n;
                    this.queue
                        .push_back((Instant::now() + this.delay, this.scratch[..n].to_vec()));
                }
                Poll::Ready(Err(e)) => this.ended = Some((Instant::now() + this.delay, Some(e))),
                Poll::Pending => break,
            }
        }
        loop {
            let release = match (this.queue.front(), &this.ended) {
                (Some((at, _)), _) => *at,
                (None, Some((at, _))) => *at,
                // The socket registered the waker when it returned Pending; with
                // the queue empty it was polled, as the limit was not reached.
                (None, None) => return Poll::Pending,
            };
            if Instant::now() < release {
                this.sleep.as_mut().reset(release);
                if this.sleep.as_mut().poll(cx).is_pending() {
                    return Poll::Pending;
                }
                continue;
            }
            if let Some((_, chunk)) = this.queue.front() {
                let n = buf.len().min(chunk.len() - this.offset);
                buf[..n].copy_from_slice(&chunk[this.offset..this.offset + n]);
                this.offset += n;
                this.queued -= n;
                if this.offset == chunk.len() {
                    this.queue.pop_front();
                    this.offset = 0;
                }
                return Poll::Ready(Ok(n));
            }
            let error = this.ended.as_mut().and_then(|(_, e)| e.take());
            return Poll::Ready(error.map_or(Ok(0), Err));
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Delayed<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::io::AsyncReadExt;

    #[test]
    fn parses_off_fixed_and_ranges() {
        assert_eq!("0".parse::<LatencyRange>().unwrap(), LatencyRange::OFF);
        assert!(LatencyRange::OFF.is_off());
        assert_eq!(
            "25".parse::<LatencyRange>().unwrap(),
            LatencyRange { min_ms: 25, max_ms: 25 }
        );
        assert_eq!(
            "10, 100".parse::<LatencyRange>().unwrap(),
            LatencyRange { min_ms: 10, max_ms: 100 }
        );
        assert!("100,10".parse::<LatencyRange>().is_err());
        assert!("fast".parse::<LatencyRange>().is_err());
    }

    /// Both ends of a link get the same delay, always within the range.
    #[test]
    fn delays_are_symmetric_and_in_range() {
        let range = LatencyRange { min_ms: 10, max_ms: 100 };
        for (a, b) in [(40000u16, 51234u16), (1, 2), (65535, 10001)] {
            let d = range.for_ports(a, b);
            assert_eq!(d, range.for_ports(b, a));
            assert!(d >= Duration::from_millis(10) && d <= Duration::from_millis(100));
        }
        assert_eq!(range.for_ports(4242, 4242), Duration::from_millis(10));
    }

    /// No detour via a third peer is faster than the direct link.
    #[test]
    fn detours_are_never_faster() {
        let range = LatencyRange { min_ms: 10, max_ms: 100 };
        let ports: Vec<u16> = (0..40).map(|i| 20000 + i * 977).collect();
        for &a in &ports {
            for &b in &ports {
                for &c in &ports {
                    let detour = range.for_ports(a, b) + range.for_ports(b, c);
                    assert!(range.for_ports(a, c) <= detour + Duration::from_micros(1));
                }
            }
        }
    }

    /// A full queue stops reading from the socket until its bytes are passed on.
    #[tokio::test]
    async fn stops_reading_when_the_queue_is_full() {
        let data: Vec<u8> = (0..3 * QUEUE_LIMIT).map(|i| i as u8).collect();
        let mut stream = Delayed::new(futures::io::Cursor::new(data.clone()), Duration::ZERO);
        let mut buf = [0u8; 10];
        stream.read_exact(&mut buf).await.unwrap();
        assert!(stream.queued < QUEUE_LIMIT + 16 * 1024);
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await.unwrap();
        assert_eq!([&buf[..], &rest[..]].concat(), data);
    }

    /// Bytes arrive in order and no earlier than the delay, then end of stream.
    #[tokio::test]
    async fn reads_lag_by_the_delay_and_keep_order() {
        let inner = futures::io::Cursor::new(b"hello world".to_vec());
        let mut stream = Delayed::new(inner, Duration::from_millis(30));
        let start = Instant::now();
        let mut out = Vec::new();
        stream.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello world");
        assert!(start.elapsed() >= Duration::from_millis(30));
    }
}
