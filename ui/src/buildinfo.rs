//! The running server's version, build time and start time.
//!
//! Home Assistant's add-on log has no timestamps or version numbers, so the server
//! logs these three values at startup and serves them, with the current time, at
//! `/version`. In the Docker image the build time comes from `BUILT_AT`, a file next
//! to the binary that the Dockerfile writes in its last step. A local `cargo` build
//! has no such file and uses the binary's mtime instead.
//!
//! Timestamps are UTC and formatted by hand (`SystemTime` -> ISO 8601) instead of with
//! `chrono` or `time`. A new dependency would invalidate the Dockerfile's dependency
//! layer, which is the slowest layer to rebuild (see the comment on that layer).

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The crate version from `Cargo.toml`. Bump it there (and in the add-on's
/// `config.yaml`) when you want a human-meaningful marker; the build time below
/// distinguishes builds either way.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The build time, as UTC ISO 8601: the contents of the `BUILT_AT` file next to the
/// executable if there is one (the Docker image has one), otherwise the executable's
/// mtime. `"unknown"` if neither can be read.
pub fn built_at() -> &'static str {
    static BUILT: OnceLock<String> = OnceLock::new();
    BUILT.get_or_init(|| {
        let Ok(exe) = std::env::current_exe() else {
            return "unknown".to_string();
        };
        std::fs::read_to_string(exe.with_file_name("BUILT_AT"))
            .ok()
            .and_then(|contents| built_at_from(&contents))
            .or_else(|| {
                std::fs::metadata(&exe)
                    .and_then(|m| m.modified())
                    .ok()
                    .map(iso8601_utc)
            })
            .unwrap_or_else(|| "unknown".to_string())
    })
}

/// The build time in a `BUILT_AT` file's contents, trimmed, or `None` if the file is
/// blank.
fn built_at_from(contents: &str) -> Option<String> {
    let trimmed = contents.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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
/// time. Open it through the sidebar or the tunnel to see which build is running.
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

    #[test]
    fn reads_built_at_file_contents() {
        assert_eq!(
            built_at_from("2026-08-05T01:23:45Z\n").as_deref(),
            Some("2026-08-05T01:23:45Z")
        );
        assert_eq!(built_at_from(" \n"), None);
        assert_eq!(built_at_from(""), None);
    }
}
