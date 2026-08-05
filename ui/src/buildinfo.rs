//! Which build is this, and how old is the log?
//!
//! An add-on log with no dates and no version can't answer "did my rebuild actually
//! take?" — so the server announces both at startup and serves them at `/version`.
//! The build time is the server binary's own mtime, which the Docker build sets when
//! it compiles: no build script, no extra dependency, and nothing to remember to bump.
//!
//! Timestamps are UTC and formatted by hand (`SystemTime` -> ISO 8601) rather than by
//! pulling in `chrono`/`time`. A new dependency would invalidate the Dockerfile's
//! dependency layer, and that layer is the expensive one — see the comment on it.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The crate version from `Cargo.toml`. Bump it there (and in the add-on's
/// `config.yaml`) when you want a human-meaningful marker; the build time below
/// distinguishes builds either way.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// When this binary was compiled, as UTC ISO 8601, or `"unknown"` if the executable's
/// mtime can't be read.
pub fn built_at() -> &'static str {
    static BUILT: OnceLock<String> = OnceLock::new();
    BUILT.get_or_init(|| {
        std::env::current_exe()
            .and_then(std::fs::metadata)
            .and_then(|m| m.modified())
            .map(iso8601_utc)
            .unwrap_or_else(|_| "unknown".to_string())
    })
}

/// When this process started, as UTC ISO 8601. Fixed on first call, so call it once
/// during startup before anything else can.
pub fn started_at() -> &'static str {
    static STARTED: OnceLock<String> = OnceLock::new();
    STARTED.get_or_init(|| iso8601_utc(SystemTime::now()))
}

/// The current time, as UTC ISO 8601.
pub fn now() -> String {
    iso8601_utc(SystemTime::now())
}

/// The body served at `/version`: version, build time, start time, and the current
/// time. Reading it through the sidebar or the tunnel confirms *which* build answered.
pub fn report() -> String {
    format!(
        "coolwords {version}\nbuilt   {built}\nstarted {started}\nnow     {now}\n",
        version = VERSION,
        built = built_at(),
        started = started_at(),
        now = now(),
    )
}

/// `2026-08-05T01:23:45Z`. Times before 1970 (a broken clock) render as the epoch.
fn iso8601_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let tod = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z",
        h = tod / 3600,
        mi = (tod % 3600) / 60,
        s = tod % 60,
    )
}

/// Days since 1970-01-01 -> (year, month, day). Hinnant's `civil_from_days`, the
/// standard branch-free calendar conversion; the shifted era starts in March so the
/// leap day lands at the end of a year and needs no special case.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era, [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year (March-based)
    let mp = (5 * doy + 2) / 153; // month, March = 0
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> String {
        iso8601_utc(UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
    }

    #[test]
    fn formats_known_instants() {
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z"); // leap day, century leap
        assert_eq!(at(1_754_352_000), "2025-08-05T00:00:00Z");
        assert_eq!(at(1_754_352_000 + 45_296), "2025-08-05T12:34:56Z");
    }
}
