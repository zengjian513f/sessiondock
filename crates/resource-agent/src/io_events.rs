//! Bounded, read-only application I/O accounting. Values are deltas per completed
//! two-second generation, never physical disk/NFS wire counters. Short-lived
//! processes remain in the completed map even after exit. Kernel/function/BTF
//! incompatibility fails this collector without affecting the workload.
use process_links::Process;
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError},
    },
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IoKind {
    TcpSend,
    TcpReceive,
    LocalRead,
    LocalWrite,
    NfsRead,
    NfsWrite,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IoSample {
    pub process: Process,
    /// Linux superblock dev_t (major = device >> 20, minor = device & 0xfffff).
    /// Zero for TCP. NFS logical I/O is keyed by its filesystem, not an endpoint.
    pub device: u64,
    pub kind: IoKind,
    pub bytes: u64,
    pub generation: u64,
}

pub enum Event {
    Ready,
    /// Completed aggregation interval, including an empty interval. generation
    /// starts at zero; missed generations mean a gap, not zero activity.
    Batch {
        generation: u64,
        samples: Vec<IoSample>,
    },
    Failed,
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum LossKind {
    InvalidJson,
    MapParse,
    UnknownStdout,
    TickDelay,
    GenerationMismatch,
    LateSample,
    QueueFull,
    Stderr,
    ReadError,
    MapCapacity,
}

struct Losses {
    total: Arc<AtomicU64>,
    counts: [AtomicU64; 10],
}

impl Losses {
    fn record(&self, kind: LossKind) {
        self.total.fetch_add(1, Ordering::Relaxed);
        if self.counts[kind as usize].fetch_add(1, Ordering::Relaxed) == 0 {
            let name = [
                "invalid_json",
                "map_parse",
                "unknown_stdout",
                "tick_delay",
                "generation_mismatch",
                "late_sample",
                "queue_full",
                "stderr",
                "read_error",
                "map_capacity",
            ][kind as usize];
            // One bounded diagnostic per category; never log tracer payloads.
            eprintln!("I/O collector loss: {name} (first occurrence)");
        }
    }
}

fn samples(value: &serde_json::Value, ticks: u64) -> Option<Vec<IoSample>> {
    let maps = value.get("data")?.as_object()?;
    let value = maps.get("@a").or_else(|| maps.get("@b"))?;
    // Some bpftrace versions represent an empty map as an empty array.
    if value.as_array().is_some_and(Vec::is_empty) {
        return Some(Vec::new());
    }
    let map = value.as_object()?;
    let mut out = Vec::with_capacity(map.len());
    for (key, value) in map {
        let fields = key
            .split(',')
            .map(str::trim)
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        if fields.len() != 5 {
            return None;
        }
        let kind = match fields[2] {
            0 => IoKind::TcpSend,
            1 => IoKind::TcpReceive,
            2 => IoKind::LocalRead,
            3 => IoKind::LocalWrite,
            4 => IoKind::NfsRead,
            5 => IoKind::NfsWrite,
            _ => return None,
        };
        out.push(IoSample {
            process: Process {
                pid: u32::try_from(fields[0]).ok()?,
                start: (fields[1] as u128 * ticks as u128 / 1_000_000_000) as u64,
            },
            device: fields[3],
            generation: fields[4],
            kind,
            bytes: value.as_u64()?,
        });
    }
    Some(out)
}

/// Caller owns the returned child and must terminate/reap it during shutdown.
/// `lost` includes stderr/helper errors, malformed data, channel overflow and
/// late generations. If nonzero, expose accounting as degraded, not exact.
/// Use a bounded receiver and keep it drained; the inactive BPF map is reused
/// after two seconds, so sustained tracer/userspace stalls can lose data.
pub fn start(uid: u32, sender: SyncSender<Event>, lost: Arc<AtomicU64>) -> std::io::Result<Child> {
    let script = include_str!("io.bt").replace("TARGET_UID", &uid.to_string());
    let mut child = Command::new("/usr/bin/bpftrace")
        // -k reports harmless delete-of-absent-key ENOENT as helper_error.
        // Cleanup probes issue these routinely; reporting them can flood stdout
        // and starve the actual samples. Keep normal tracer loss diagnostics.
        .args(["-q", "-f", "json", "-B", "line", "-e", &script])
        .env("BPFTRACE_MAX_MAP_KEYS", "4096")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let losses = Arc::new(Losses {
        total: lost,
        counts: std::array::from_fn(|_| AtomicU64::new(0)),
    });
    let errors = losses.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => errors.record(LossKind::Stderr),
                Ok(_) => {}
                Err(_) => {
                    errors.record(LossKind::ReadError);
                    break;
                }
            }
        }
    });
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    std::thread::spawn(move || {
        let mut pending = Vec::new();
        let mut next = 0;
        let mut last_tick = std::time::Instant::now();
        let send = |event| match sender.try_send(event) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                losses.record(LossKind::QueueFull);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        };
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => {
                    losses.record(LossKind::ReadError);
                    break;
                }
            }
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                losses.record(LossKind::InvalidJson);
                continue;
            };
            match value.get("type").and_then(|v| v.as_str()) {
                Some("map") => match samples(&value, ticks) {
                    Some(rows) => {
                        if rows.len() >= 4096 {
                            losses.record(LossKind::MapCapacity);
                        }
                        pending.extend(rows);
                    }
                    None => {
                        losses.record(LossKind::MapParse);
                    }
                },
                Some("printf") => {
                    let data = value
                        .get("data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if data == "READY" {
                        last_tick = std::time::Instant::now();
                        if !send(Event::Ready) {
                            return;
                        }
                    } else if let Some(generation) = data
                        .strip_prefix("TICK ")
                        .and_then(|v| v.parse::<u64>().ok())
                        .and_then(|v| v.checked_sub(1))
                    {
                        if last_tick.elapsed().as_secs_f64() > 3.0 {
                            losses.record(LossKind::TickDelay);
                        }
                        last_tick = std::time::Instant::now();
                        if generation != next {
                            losses.record(LossKind::GenerationMismatch);
                        }
                        next = generation + 1;
                        pending.retain(|sample| {
                            if sample.generation == generation {
                                true
                            } else {
                                losses.record(LossKind::LateSample);
                                false
                            }
                        });
                        if !send(Event::Batch {
                            generation,
                            samples: std::mem::take(&mut pending),
                        }) {
                            return;
                        }
                    } else {
                        losses.record(LossKind::UnknownStdout);
                    }
                }
                _ => {
                    losses.record(LossKind::UnknownStdout);
                }
            }
        }
        let _ = sender.send(Event::Failed);
    });
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_incarnations_and_filesystems_are_preserved() {
        let value = serde_json::json!({"type":"map","data":{"@a":{
            "12,123456789,4,77,0":8192,"12,223456789,5,78,0":3
        }}});
        let rows = samples(&value, 100).unwrap();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].process.start, rows[1].process.start);
        assert_ne!(rows[0].device, rows[1].device);
        assert!(
            rows.iter()
                .any(|v| v.kind == IoKind::NfsRead && v.bytes == 8192)
        );
    }
    #[test]
    fn empty_maps_are_completed_empty_intervals() {
        for empty in [serde_json::json!({}), serde_json::json!([])] {
            assert!(
                samples(&serde_json::json!({"data":{"@a":empty}}), 100)
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(samples(&serde_json::json!({"data":{"@a":[1]}}), 100).is_none());
    }

    #[test]
    fn malformed_samples_are_not_zeroes() {
        assert!(samples(&serde_json::json!({"data":{"@b":{"12,1,99,0,0":1}}}), 100).is_none());
        assert!(samples(&serde_json::json!({"data":{"@b":{"12,1,0,0,0":-1}}}), 100).is_none());
    }
}
