//! Background-job registry: long-running Python subprocesses (OCR, reingest,
//! trajectory) run ONE AT A TIME with live progress + cancel, for the book manage
//! page. Server fns can't see leptos context, so the registry is a global static
//! (server fns are plain async fns in the axum/tokio process and can touch it).
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

/// Snapshot for the client; also reaps jobs that finished > 10 min ago.
pub fn status(id: &str) -> Option<JobProgress> {
    let mut jobs = JOBS.lock().ok()?;
    let cutoff = now().saturating_sub(600);
    jobs.retain(|_, j| {
        matches!(j.prog.status.as_str(), "queued" | "running") || j.prog.updated >= cutoff
    });
    jobs.get(id).map(|j| j.prog.clone())
}

pub fn cancel(id: &str) {
    let pid = {
        let mut jobs = match JOBS.lock() {
            Ok(j) => j,
            Err(_) => return,
        };
        match jobs.get_mut(id) {
            Some(j) => {
                j.cancel.store(true, Ordering::Relaxed);
                j.prog.status = "cancelled".into();
                j.prog.updated = now();
                j.pid
            }
            None => return,
        }
    };
    if let Some(pid) = pid {
        tree_kill(pid);
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
    tokio::spawn(run(id.clone(), kind, args, cancel));
    id
}

async fn run(id: String, kind: String, args: Vec<String>, cancel: Arc<AtomicBool>) {
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
        .arg("ingest.import_book")
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
    let out = out_task.await.unwrap_or_default();
    let ok_json = serde_json::from_str::<serde_json::Value>(out.trim().lines().last().unwrap_or(""))
        .ok()
        .and_then(|v| v.get("ok").and_then(|b| b.as_bool()));
    let err_msg = serde_json::from_str::<serde_json::Value>(out.trim().lines().last().unwrap_or(""))
        .ok()
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
        _ => None,
    }
}
