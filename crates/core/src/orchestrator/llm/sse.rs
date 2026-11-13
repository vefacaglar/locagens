//! SSE line buffering and idle-timeout chunk reading.

use super::*;

/// Longest silence tolerated between two stream chunks. Replaces the old
/// 600 s *total* request timeout, which killed long reasoning streams.
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// How often `should_stop` is polled while waiting for the next chunk.
pub(super) const STOP_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Raw SSE bytes split into lines on `b'\n'` *before* UTF-8 decoding, so a
/// multibyte character split across two network chunks stays intact.
#[derive(Default)]
pub struct SseLineBuffer {
    pending: Vec<u8>,
}

impl SseLineBuffer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// The next complete line (without `\n` / `\r\n`), if one is buffered.
    pub fn next_line(&mut self) -> Option<String> {
        let newline = self.pending.iter().position(|byte| *byte == b'\n')?;
        let mut line: Vec<u8> = self.pending.drain(..=newline).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Some(String::from_utf8_lossy(&line).into_owned())
    }
}

/// Reads the next stream chunk, failing after [`STREAM_IDLE_TIMEOUT`] of
/// silence and returning `"cancelled"` as soon as `should_stop` fires (it is
/// polled while waiting, not only when a chunk arrives).
pub(super) async fn next_stream_chunk<S, B>(
    stream: &mut S,
    should_stop: &(dyn Fn() -> bool + Send + Sync),
) -> Result<Option<B>>
where
    S: futures::Stream<Item = reqwest::Result<B>> + Unpin,
{
    next_stream_chunk_within(stream, should_stop, STREAM_IDLE_TIMEOUT).await
}

pub(super) async fn next_stream_chunk_within<S, B, E>(
    stream: &mut S,
    should_stop: &(dyn Fn() -> bool + Send + Sync),
    idle_timeout: Duration,
) -> Result<Option<B>>
where
    S: futures::Stream<Item = std::result::Result<B, E>> + Unpin,
    E: std::fmt::Display,
{
    use futures::StreamExt;
    let started = tokio::time::Instant::now();
    loop {
        if should_stop() {
            anyhow::bail!("cancelled");
        }
        match tokio::time::timeout(STOP_POLL_INTERVAL, stream.next()).await {
            Ok(None) => return Ok(None),
            Ok(Some(chunk)) => {
                if should_stop() {
                    anyhow::bail!("cancelled");
                }
                return chunk.map(Some).map_err(|err| anyhow::anyhow!("Stream interrupted: {err}"));
            }
            Err(_) if started.elapsed() >= idle_timeout => {
                anyhow::bail!("Stream stalled: no data from the provider for {} s", idle_timeout.as_secs())
            }
            Err(_) => {}
        }
    }
}
