//! Background-job registry: long-running Python subprocesses (OCR, reingest,
//! trajectory, catalog sync, bulk grab) run ONE AT A TIME with live progress +
//! cancel, for the book manage page. Server fns can't see leptos context, so the
//! registry is a global static (server fns are plain async fns in the axum/tokio
//! process and can touch it).
//!
//! Any `ingest.*` module can be driven — see `start_module`. Every one of them
//! keeps the same contract: human progress lines on stderr, exactly one JSON object
//! on stdout.
//!
//! Cancel = set a flag + TREE-kill the OS process (on Windows, killing python alone
//! leaves its tesseract child orphaned — taskkill /T takes the whole tree).
#![cfg(feature = "ssr")]

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::Semaphore;

use crate::app::{python_exe, repo_root, JobProgress};

struct Job {
    prog: JobProgress,
    pid: Option<u32>,
    cancel: Arc<AtomicBool>,
}

static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// One job at a time — kind to the CPU and predictable. Extra jobs wait here (queued).
static GATE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(1));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn set<F: FnOnce(&mut JobProgress)>(id: &str, f: F) {
    if let Ok(mut jobs) = JOBS.lock() {
        if let Some(j) = jobs.get_mut(id) {
            f(&mut j.prog);
            j.prog.updated = now();
        }
    }
}

/// A queued/running job matching (book_id, kind, tag) — so a duplicate request
/// returns the existing job instead of starting a second one.
fn find_live(book_id: i64, kind: &str, tag: &str) -> Option<String> {
    let jobs = JOBS.lock().ok()?;
    jobs.values()
        .find(|j| {
            j.prog.book_id == book_id
                && j.prog.kind == kind
                && j.prog.tag == tag
                && matches!(j.prog.status.as_str(), "queued" | "running")
        })
        .map(|j| j.prog.id.clone())
}

/// Drop jobs that finished > 10 min ago. Callers already hold the lock.
fn reap(jobs: &mut HashMap<String, Job>) {
    let cutoff = now().saturating_sub(600);
    jobs.retain(|_, j| {
        matches!(j.prog.status.as_str(), "queued" | "running") || j.prog.updated >= cutoff
    });
}

/// Snapshot for the client; also reaps jobs that finished > 10 min ago.
pub fn status(id: &str) -> Option<JobProgress> {
    let mut jobs = JOBS.lock().ok()?;
    reap(&mut jobs);
    jobs.get(id).map(|j| j.prog.clone())
}

/// Every live/recent job, for the queue panel on /get and /books. Same reaping rule
/// as `status`; running first, then most-recently-updated, so the panel's top row
/// is always the thing actually happening right now.
pub fn list() -> Vec<JobProgress> {
    let mut jobs = match JOBS.lock() {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };
    reap(&mut jobs);
    let mut out: Vec<JobProgress> = jobs.values().map(|j| j.prog.clone()).collect();
    out.sort_by(|a, b| {
        let rank = |s: &str| match s {
            "running" => 0,
            "queued" => 1,
            _ => 2,
        };
        rank(&a.status)
            .cmp(&rank(&b.status))
            .then(b.updated.cmp(&a.updated))
    });
    out
}

pub fn cancel(id: &str) {
    let pid = {
        let mut jobs = match JOBS.lock() {
            Ok(j) => j,
            Err(_) => return,
        };
        match jobs.get_mut(id) {
            // Only a LIVE job can be cancelled. A finished one sits in the map for
            // another 10 minutes (see `reap`) and the client's view is up to a poll
            // interval stale, so without this guard a just-completed job would be
            // relabelled "cancelled" — and, worse, `taskkill /T /F` would fire at a
            // pid the OS may already have handed to somebody else's process tree.
            // `.take()` so the pid can never be killed twice.
            Some(j) if matches!(j.prog.status.as_str(), "queued" | "running") => {
                j.cancel.store(true, Ordering::Relaxed);
                j.prog.status = "cancelled".into();
                j.prog.updated = now();
                j.pid.take()
            }
            _ => return,
        }
    };
    if let Some(pid) = pid {
        tree_kill(pid);
    }
}

/// Stop everything of one kind — the bulk grab's panic button. Collect the ids
/// first so we're not holding the registry lock while `cancel` re-takes it.
pub fn cancel_all(kind: &str) {
    let ids: Vec<String> = match JOBS.lock() {
        Ok(jobs) => jobs
            .values()
            .filter(|j| {
                j.prog.kind == kind && matches!(j.prog.status.as_str(), "queued" | "running")
            })
            .map(|j| j.prog.id.clone())
            .collect(),
        Err(_) => return,
    };
    for id in ids {
        cancel(&id);
    }
}

/// Kill an OS process and ALL its descendants (the python parent + its tesseract
/// child). On Windows `taskkill /T`; elsewhere best-effort SIGKILL.
fn tree_kill(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

/// Start a background job running `python -m ingest.import_book <args>`. Returns the
/// (existing, if duplicate) job id immediately; the work proceeds on a tokio task.
pub fn start(kind: &str, book_id: i64, tag: &str, label: &str, args: Vec<String>) -> String {
    start_module(kind, "ingest.import_book", book_id, tag, label, args)
}

/// The general form: run ANY `python -m <module> <args>` under the same one-at-a-time
/// gate, progress parsing and cancel semantics. `kind` still selects the
/// `parse_progress` arm and (with book_id + tag) the duplicate-request key, so a
/// module can be driven under several kinds if its progress lines differ.
pub fn start_module(
    kind: &str,
    module: &str,
    book_id: i64,
    tag: &str,
    label: &str,
    args: Vec<String>,
) -> String {
    if let Some(existing) = find_live(book_id, kind, tag) {
        return existing;
    }
    let id = format!("job-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut jobs) = JOBS.lock() {
        jobs.insert(
            id.clone(),
            Job {
                prog: JobProgress {
                    id: id.clone(),
                    kind: kind.to_string(),
                    book_id,
                    tag: tag.to_string(),
                    status: "queued".into(),
                    percent: -1.0,
                    message: label.to_string(),
                    updated: now(),
                },
                pid: None,
                cancel: cancel.clone(),
            },
        );
    }
    let kind = kind.to_string();
    let module = module.to_string();
    tokio::spawn(run(id.clone(), kind, module, args, cancel));
    id
}

async fn run(
    id: String,
    kind: String,
    module: String,
    args: Vec<String>,
    cancel: Arc<AtomicBool>,
) {
    // wait our turn (one job at a time)
    let _permit = GATE.acquire().await;
    if cancel.load(Ordering::Relaxed) {
        set(&id, |p| p.status = "cancelled".into());
        return;
    }
    set(&id, |p| {
        p.status = "running".into();
        p.message = "starting…".into();
    });

    let mut child = match tokio::process::Command::new(python_exe())
        .current_dir(repo_root())
        .arg("-m")
        .arg(&module)
        .args(&args)
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            set(&id, |p| {
                p.status = "failed".into();
                p.message = format!("could not start python: {e}");
            });
            return;
        }
    };
    let pid = child.id();
    set(&id, |p| p.percent = if p.percent < 0.0 { 0.0 } else { p.percent });
    if let Ok(mut jobs) = JOBS.lock() {
        if let Some(j) = jobs.get_mut(&id) {
            j.pid = pid;
        }
    }

    // read final JSON (stdout) concurrently so the pipe never blocks the child
    let stdout = child.stdout.take();
    let out_task = tokio::spawn(async move {
        let mut s = String::new();
        if let Some(o) = stdout {
            let _ = BufReader::new(o).read_to_string(&mut s).await;
        }
        s
    });

    // stream stderr lines -> progress; the loop ends when the child exits (or is killed)
    if let Some(err) = child.stderr.take() {
        let mut lines = BufReader::new(err).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some((pct, msg)) = parse_progress(&kind, &line) {
                set(&id, |p| {
                    if pct >= 0.0 {
                        p.percent = pct;
                    }
                    p.message = msg;
                });
            }
        }
    }

    let exit = child.wait().await;
    // The child is reaped, so the pid is dead and the OS is free to reissue it —
    // drop it before anything can cancel this job on its way to a terminal status.
    if let Ok(mut jobs) = JOBS.lock() {
        if let Some(j) = jobs.get_mut(&id) {
            j.pid = None;
        }
    }
    let out = out_task.await.unwrap_or_default();
    // Every ingest module's contract: the LAST stdout line is its single JSON object.
    // Parse it once and read both fields off it.
    let payload =
        serde_json::from_str::<serde_json::Value>(out.trim().lines().last().unwrap_or("")).ok();
    let ok_json = payload.as_ref().and_then(|v| v.get("ok").and_then(|b| b.as_bool()));
    let err_msg = payload
        .as_ref()
        .and_then(|v| v.get("error").and_then(|s| s.as_str()).map(str::to_string));

    if cancel.load(Ordering::Relaxed) {
        set(&id, |p| {
            p.status = "cancelled".into();
            p.message = "cancelled".into();
        });
    } else if matches!(exit, Ok(s) if s.success()) && ok_json != Some(false) {
        set(&id, |p| {
            p.status = "done".into();
            p.percent = 100.0;
            p.message = "done".into();
        });
    } else {
        set(&id, |p| {
            p.status = "failed".into();
            p.message = err_msg.clone().unwrap_or_else(|| "failed".into());
        });
    }
}

/// Map a python stderr line to (percent, message). percent < 0 ⇒ leave unchanged.
fn parse_progress(kind: &str, line: &str) -> Option<(f32, String)> {
    match kind {
        // "ocr[tesseract] page 12 (12/277)"
        "ocr" => {
            let frac = line.rsplit_once('(')?.1.split_once(')')?.0; // "12/277"
            let (i, n) = frac.split_once('/')?;
            let (i, n): (f32, f32) = (i.trim().parse().ok()?, n.trim().parse().ok()?);
            let pct = if n > 0.0 { i / n * 100.0 } else { -1.0 };
            Some((pct, format!("OCR page {} / {}", i as i64, n as i64)))
        }
        // "reingest: score"
        "reingest" => {
            let step = line.strip_prefix("reingest: ")?.trim();
            let pct = match step {
                s if s.starts_with("extract") => 15.0,
                s if s.starts_with("ingest") => 35.0,
                s if s.starts_with("score") => 60.0,
                s if s.starts_with("cluster") => 85.0,
                _ => -1.0,
            };
            Some((pct, format!("re-ingesting: {step}")))
        }
        // "trajectory: refreshing usage charts" (indeterminate)
        "trajectory" => line
            .strip_prefix("trajectory: ")
            .map(|m| (-1.0, format!("usage charts: {}", m.trim()))),
        // "catalog: standardebooks page 7/31" (determinate — SE is paged HTML) and
        // "catalog: gutenberg 42000 rows" (indeterminate — one streamed CSV).
        "catalog" => {
            let (src, tail) = line.strip_prefix("catalog: ")?.trim().split_once(' ')?;
            if let Some(frac) = tail.strip_prefix("page ") {
                let (i, n) = frac.trim().split_once('/')?;
                let (i, n): (f32, f32) = (i.trim().parse().ok()?, n.trim().parse().ok()?);
                let pct = if n > 0.0 { i / n * 100.0 } else { -1.0 };
                Some((pct, format!("{src}: page {} / {}", i as i64, n as i64)))
            } else {
                let n: i64 = tail.strip_suffix("rows")?.trim().parse().ok()?;
                Some((-1.0, format!("{src}: {} rows", thousands(n))))
            }
        }
        // "grab: 3/25 The Mystery of Edwin Drood" (determinate) and, per finished
        // book, "grab: imported|failed|skipped …" plus a closing "grab: done …" —
        // notes only, so they leave percent alone.
        "grab" => {
            let rest = line.strip_prefix("grab: ")?.trim();
            // Notes rather than counters — they carry no i/n, so they only set the
            // message. "failed"/"skipped" have to be recognised explicitly: falling
            // through would leave `frac` = "failed", the split on '/' would fail, and
            // a whole batch of failures would scroll past without one visible word.
            for p in ["imported ", "failed ", "skipped ", "done "] {
                if let Some(tail) = rest.strip_prefix(p) {
                    return Some((-1.0, format!("{p}{}", tail.trim())));
                }
            }
            let (frac, title) = rest.split_once(' ').unwrap_or((rest, ""));
            let (i, n) = frac.split_once('/')?;
            let (i, n): (f32, f32) = (i.trim().parse().ok()?, n.trim().parse().ok()?);
            let pct = if n > 0.0 { i / n * 100.0 } else { -1.0 };
            Some((pct, format!("{} / {} {}", i as i64, n as i64, title).trim_end().to_string()))
        }
        // "rescore: score" — the two heavy steps of a library-wide re-score.
        "rescore" => {
            let step = line.strip_prefix("rescore: ")?.trim();
            let pct = match step {
                s if s.starts_with("score") => 40.0,
                s if s.starts_with("cluster") => 80.0,
                _ => -1.0,
            };
            Some((pct, format!("re-scoring: {step}")))
        }
        _ => None,
    }
}

/// 42000 -> "42,000". Row counts stream past fast enough that unseparated digits
/// are unreadable.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
