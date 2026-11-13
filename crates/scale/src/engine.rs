//! Background reader: keeps the link open (reconnecting forever), frames and
//! parses the stream, applies stability, and publishes the live state.

use crate::profile::{escape, CompiledProfile, Profile};
use crate::stability::{Stability, StabilityConfig};
use crate::transport::Transport;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScaleConfig {
    pub transport: Transport,
    pub profile: Profile,
    #[serde(default)]
    pub stability: StabilityConfig,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reading {
    pub weight_kg: f64,
    /// Final verdict used for capture: indicator flag AND our own window check.
    pub stable: bool,
    /// What the indicator itself reported, if anything.
    pub indicator_stable: Option<bool>,
    /// The frame exactly as received (escaped), stored on every ticket.
    pub raw: String,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScaleState {
    pub link: String,
    pub connected: bool,
    /// True while valid frames keep arriving (false after 3 s of silence).
    pub receiving: bool,
    pub error: Option<String>,
    pub reading: Option<Reading>,
    pub frames_ok: u64,
    pub frames_bad: u64,
}

pub struct ScaleHandle {
    pub state: watch::Receiver<ScaleState>,
    raw: Arc<Mutex<VecDeque<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
}

impl ScaleHandle {
    /// Last raw chunks received (for diagnostics and Learn mode).
    pub fn raw_bytes(&self) -> Vec<u8> {
        self.raw.lock().unwrap().iter().flatten().copied().collect()
    }
    pub fn current(&self) -> ScaleState {
        self.state.borrow().clone()
    }
}

impl Drop for ScaleHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub fn start(cfg: ScaleConfig) -> anyhow::Result<ScaleHandle> {
    let compiled = cfg.profile.compile()?;
    let (tx, rx) = watch::channel(ScaleState { link: cfg.transport.label(), ..Default::default() });
    let raw = Arc::new(Mutex::new(VecDeque::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (raw2, stop2) = (raw.clone(), stop.clone());
    std::thread::Builder::new()
        .name("scale-reader".into())
        .spawn(move || run(cfg, compiled, tx, raw2, stop2))?;
    Ok(ScaleHandle { state: rx, raw, stop })
}

fn run(
    cfg: ScaleConfig,
    profile: CompiledProfile,
    tx: watch::Sender<ScaleState>,
    raw: Arc<Mutex<VecDeque<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
) {
    // Some indicators only answer when asked ("demand mode"). Their profile
    // carries a poll command that we send every `poll_interval_ms`.
    let poll = cfg.profile.poll_bytes();
    let poll_every = Duration::from_millis(cfg.profile.poll_interval_ms.max(50));
    // Outer loop: (re)connect forever. A pulled cable, an unplugged USB
    // adapter or an indicator switched off all end up here, and we simply
    // try again every 2 seconds until the link comes back.
    while !stop.load(Ordering::SeqCst) {
        let mut link = match cfg.transport.open(Duration::from_millis(200)) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("scale link {}: {e}", cfg.transport.label());
                tx.send_modify(|s| {
                    s.connected = false;
                    s.receiving = false;
                    s.error = Some(e.to_string());
                });
                sleep_unless_stopped(&stop, Duration::from_secs(2));
                continue;
            }
        };
        tracing::info!("scale link open: {}", cfg.transport.label());
        tx.send_modify(|s| {
            s.connected = true;
            s.error = None;
        });
        let mut stability = Stability::new(cfg.stability.clone());
        // `pending` holds bytes received but not yet ending in a separator
        // (a frame cut in half between two reads).
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = [0u8; 512];
        let mut last_frame = Instant::now();
        let mut last_poll = Instant::now() - poll_every;

        loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if let Some(cmd) = &poll {
                if last_poll.elapsed() >= poll_every {
                    let _ = link.write_all(cmd);
                    last_poll = Instant::now();
                }
            }
            // Read whatever has arrived (the link has a 200 ms timeout, so this
            // never blocks for long and the stop flag is checked often).
            match link.read(&mut buf) {
                Ok(0) => {
                    tx.send_modify(|s| {
                        s.connected = false;
                        s.receiving = false;
                        s.error = Some("link closed".into());
                    });
                    break;
                }
                Ok(n) => {
                    {
                        let mut r = raw.lock().unwrap();
                        r.push_back(buf[..n].to_vec());
                        while r.iter().map(|c| c.len()).sum::<usize>() > 4096 {
                            r.pop_front();
                        }
                    }
                    // Keep a rolling copy of the raw bytes for "Show raw data" and Learn mode.
                    // Then append to the pending buffer and cut out complete frames.
                    pending.extend_from_slice(&buf[..n]);
                    let (frames, tail) = profile.split(&pending);
                    let mut frames: Vec<Vec<u8>> = frames.into_iter().map(|f| f.to_vec()).collect();
                    let mut tail = tail.to_vec();
                    // Indicators with no separators at all: treat a long run as a frame.
                    if tail.len() > 128 {
                        frames.push(std::mem::take(&mut tail));
                    }
                    pending = tail;
                    for f in frames {
                        let now = Instant::now();
                        // Turn one frame into a weight. Frames that don't match the
                        // profile (noise, checksums, half frames) are counted and ignored.
                        match profile.parse(&f) {
                            Some(p) => {
                                last_frame = now;
                                let stable = stability.push(now, p.weight_kg, p.stable);
                                let reading = Reading {
                                    weight_kg: p.weight_kg,
                                    stable,
                                    indicator_stable: p.stable,
                                    raw: escape(&f),
                                    at: Utc::now(),
                                };
                                tx.send_modify(|s| {
                                    s.receiving = true;
                                    s.frames_ok += 1;
                                    s.reading = Some(reading);
                                });
                            }
                            None => tx.send_modify(|s| s.frames_bad += 1),
                        }
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(e) => {
                    tracing::warn!("scale read error: {e}");
                    tx.send_modify(|s| {
                        s.connected = false;
                        s.receiving = false;
                        s.error = Some(e.to_string());
                    });
                    break;
                }
            }
            // No valid frame for 3 s: the indicator stopped talking (or the
            // protocol is wrong). Mark the reading as not live and not stable,
            // so nobody can capture a stale weight.
            if last_frame.elapsed() > Duration::from_secs(3) {
                tx.send_if_modified(|s| {
                    let changed = s.receiving;
                    s.receiving = false;
                    if let Some(r) = s.reading.as_mut() {
                        r.stable = false;
                    }
                    changed
                });
            }
        }
        sleep_unless_stopped(&stop, Duration::from_secs(1));
    }
}

fn sleep_unless_stopped(stop: &AtomicBool, d: Duration) {
    let until = Instant::now() + d;
    while Instant::now() < until && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
}
