//! RFC 8628 device-authorization poller (port of pi's `device-code.js`).
//!
//! Cancellation: drop the returned future.

use anyhow::{bail, Result};
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

const TIMEOUT_MESSAGE: &str = "Device flow timed out";
const SLOW_DOWN_TIMEOUT_MESSAGE: &str = "Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.";

/// Outcome of one poll request.
pub enum PollStep<T> {
    Pending,
    /// The server asks to poll less often; `interval_secs` is the new
    /// required interval when the server reports one.
    SlowDown { interval_secs: Option<f64> },
    Complete(T),
    Failed(String),
}

/// Poll pacing. The defaults follow RFC 8628; tests shrink them.
#[derive(Clone, Copy, Debug)]
pub struct PollTiming {
    /// Used when the server omits `interval` (RFC 8628 section 3.2).
    pub default_interval: Duration,
    /// Floor for every interval.
    pub min_interval: Duration,
    /// Added on `slow_down` without a server interval (RFC 8628 section 3.5).
    pub slow_down_step: Duration,
}

impl Default for PollTiming {
    fn default() -> Self {
        Self {
            default_interval: Duration::from_secs(5),
            min_interval: Duration::from_secs(1),
            slow_down_step: Duration::from_secs(5),
        }
    }
}

/// Polls until the user completes (or refuses) the authorization.
/// Waits one interval before the first poll.
pub async fn poll_device_code<T, F, Fut>(
    interval_secs: Option<f64>,
    expires_in_secs: f64,
    timing: PollTiming,
    mut poll: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<PollStep<T>>>,
{
    let deadline = Instant::now() + Duration::from_secs_f64(expires_in_secs.max(0.0));
    let mut interval = interval_secs
        .and_then(positive_duration)
        .unwrap_or(timing.default_interval)
        .max(timing.min_interval);
    let mut slow_downs = 0u32;

    sleep_until_or_deadline(interval, deadline).await;
    while Instant::now() < deadline {
        match poll().await? {
            PollStep::Complete(value) => return Ok(value),
            PollStep::Failed(message) => bail!(message),
            PollStep::Pending => {}
            PollStep::SlowDown { interval_secs } => {
                slow_downs += 1;
                interval = match interval_secs.and_then(positive_duration) {
                    Some(server) => server.max(timing.min_interval),
                    None => (interval + timing.slow_down_step).max(timing.min_interval),
                };
            }
        }
        sleep_until_or_deadline(interval, deadline).await;
    }
    bail!(if slow_downs > 0 { SLOW_DOWN_TIMEOUT_MESSAGE } else { TIMEOUT_MESSAGE })
}

fn positive_duration(secs: f64) -> Option<Duration> {
    (secs.is_finite() && secs > 0.0).then(|| Duration::from_secs_f64(secs))
}

async fn sleep_until_or_deadline(interval: Duration, deadline: Instant) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    tokio::time::sleep(interval.min(remaining)).await;
}
