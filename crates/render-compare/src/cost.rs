//! What a render costs: wall time, CPU seconds (this process and the children it waited for) and
//! the peak memory of the process tree, sampled from `ps` (the sum of resident sizes, which counts
//! a shared page once per process: an upper bound).

use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nix::sys::resource::{UsageWho, getrusage};

/// The measure of one run.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cost {
    pub wall_s: f64,
    pub cpu_s: f64,
    pub peak_rss_mib: f64,
}

impl Cost {
    /// CPU seconds per minute of video.
    pub fn core_seconds_per_media_minute(&self, media_s: f64) -> f64 {
        if media_s > 0.0 {
            self.cpu_s / (media_s / 60.0)
        } else {
            0.0
        }
    }
}

/// The CPU seconds the container's cgroup has used (all processes in it, children never waited for
/// included), when the kernel says.
fn cgroup_cpu_s() -> Option<f64> {
    let stat = std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").ok()?;
    parse_cgroup_cpu(&stat)
}

fn parse_cgroup_cpu(stat: &str) -> Option<f64> {
    stat.lines()
        .find_map(|l| l.strip_prefix("usage_usec "))
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|us| us / 1e6)
}

/// CPU seconds so far: the cgroup's where there is one (Chromium's children are not waited for, so
/// `getrusage` misses most of its work), else this process's and its waited-for children's.
fn cpu_now() -> f64 {
    cgroup_cpu_s().unwrap_or_else(rusage_cpu_s)
}

fn rusage_cpu_s() -> f64 {
    let seconds = |what| {
        getrusage(what)
            .map(|u| {
                let t = |v: nix::sys::time::TimeVal| v.tv_sec() as f64 + v.tv_usec() as f64 / 1e6;
                t(u.user_time()) + t(u.system_time())
            })
            .unwrap_or(0.0)
    };
    seconds(UsageWho::RUSAGE_SELF) + seconds(UsageWho::RUSAGE_CHILDREN)
}

/// The resident size (KiB) of this process and everything below it.
fn tree_rss_kib(root: u32) -> u64 {
    let Ok(out) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss="])
        .output()
    else {
        return 0;
    };
    parse_tree(&String::from_utf8_lossy(&out.stdout), root)
}

fn parse_tree(ps: &str, root: u32) -> u64 {
    let rows: Vec<(u32, u32, u64)> = ps
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some((
                f.next()?.parse().ok()?,
                f.next()?.parse().ok()?,
                f.next()?.parse().ok()?,
            ))
        })
        .collect();
    let children: HashMap<u32, Vec<usize>> =
        rows.iter()
            .enumerate()
            .fold(HashMap::new(), |mut m, (i, r)| {
                m.entry(r.1).or_default().push(i);
                m
            });
    let (mut total, mut stack) = (0, vec![root]);
    while let Some(pid) = stack.pop() {
        total += rows.iter().filter(|r| r.0 == pid).map(|r| r.2).sum::<u64>();
        stack.extend(children.get(&pid).into_iter().flatten().map(|i| rows[*i].0));
    }
    total
}

/// Measures from its creation to [`Meter::finish`].
#[derive(Debug)]
pub struct Meter {
    start: Instant,
    cpu: f64,
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    sampler: Option<JoinHandle<()>>,
}

impl Meter {
    pub fn start() -> Self {
        let (stop, peak) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
        );
        let (s, p) = (stop.clone(), peak.clone());
        let sampler = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(tree_rss_kib(std::process::id()), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        Self {
            start: Instant::now(),
            cpu: cpu_now(),
            stop,
            peak,
            sampler: Some(sampler),
        }
    }

    pub fn finish(mut self) -> Cost {
        let wall_s = self.start.elapsed().as_secs_f64();
        let cpu_s = (cpu_now() - self.cpu).max(0.0);
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.sampler.take() {
            let _ = handle.join();
        }
        Cost {
            wall_s,
            cpu_s,
            peak_rss_mib: self.peak.load(Ordering::Relaxed) as f64 / 1024.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cgroup_reports_microseconds() {
        let stat = "usage_usec 12500000\nuser_usec 10000000\nsystem_usec 2500000\n";
        assert_eq!(parse_cgroup_cpu(stat), Some(12.5));
        assert_eq!(parse_cgroup_cpu("nr_periods 0\n"), None);
    }

    #[test]
    fn the_tree_sums_a_process_and_its_descendants() {
        let ps = "  1 0 100\n 10 1 200\n 11 10 300\n 12 10 400\n 20 1 999\n";
        assert_eq!(parse_tree(ps, 10), 200 + 300 + 400);
        assert_eq!(parse_tree(ps, 99), 0);
    }

    #[test]
    fn cost_per_media_minute() {
        let c = Cost {
            wall_s: 10.0,
            cpu_s: 30.0,
            peak_rss_mib: 1.0,
        };
        assert_eq!(c.core_seconds_per_media_minute(30.0), 60.0);
        assert_eq!(c.core_seconds_per_media_minute(0.0), 0.0);
    }

    #[test]
    fn a_meter_sees_time_and_memory() {
        let meter = Meter::start();
        let _hold = vec![1u8; 8 << 20];
        std::thread::sleep(Duration::from_millis(300));
        let cost = meter.finish();
        assert!(cost.wall_s >= 0.3 && cost.peak_rss_mib > 1.0, "{cost:?}");
    }
}
