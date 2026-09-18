//! One rotating log file next to stderr, see `specs/0015-call-diagnostics.md`.
//!
//! A packaged app has no terminal, so `eprintln!` alone is invisible exactly
//! when a live call misbehaves.

use std::fmt::Arguments;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// The current file is rotated to `<name>.1` once it passes this.
const MAX_BYTES: u64 = 1024 * 1024;

struct Sink {
    path: PathBuf,
    file: File,
    written: u64,
}

impl Sink {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path,
            file,
            written,
        })
    }

    fn write(&mut self, line: &str) -> std::io::Result<()> {
        if self.written + line.len() as u64 > MAX_BYTES {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.written += line.len() as u64;
        Ok(())
    }

    /// Keeps one previous file: enough to see what led to a failed call.
    fn rotate(&mut self) -> std::io::Result<()> {
        let previous = self.path.with_extension("log.1");
        std::fs::rename(&self.path, &previous)?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

fn sink() -> &'static Mutex<Option<Sink>> {
    static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(None))
}

/// Starts writing to `path`; without it lines only reach stderr.
pub fn init(path: &Path) {
    match Sink::open(path.to_path_buf()) {
        Ok(opened) => *sink().lock().unwrap() = Some(opened),
        Err(err) => eprintln!("log file {} unavailable: {err}", path.display()),
    }
}

/// `ASTER_LOG=debug` adds the noisy lines, such as every ICE candidate.
pub fn debug_enabled() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("ASTER_LOG").is_ok_and(|level| level.eq_ignore_ascii_case("debug"))
    })
}

pub fn write(args: Arguments<'_>) {
    let line = format!("{} {args}\n", timestamp(now_secs()));
    eprint!("{line}");
    if let Some(sink) = sink().lock().unwrap().as_mut() {
        if let Err(err) = sink.write(&line) {
            eprintln!("log file write failed: {err}");
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after 1970")
        .as_secs()
}

/// `2026-09-18T07:04:11Z` without pulling in a date crate.
fn timestamp(secs: u64) -> String {
    let (days, time) = (secs / 86_400, secs % 86_400);
    let (hour, minute, second) = (time / 3600, (time % 3600) / 60, time % 60);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Writes one line to the log file and stderr.
#[macro_export]
macro_rules! log_line {
    ($($arg:tt)*) => {
        $crate::logging::write(format_args!($($arg)*))
    };
}

/// Like [`log_line!`], but only with `ASTER_LOG=debug`.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        if $crate::logging::debug_enabled() {
            $crate::logging::write(format_args!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps_as_utc() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(1_758_178_800), "2025-09-18T07:00:00Z");
        // A leap day, to catch an off-by-one in the civil-date maths.
        assert_eq!(timestamp(1_709_208_000), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn rotates_once_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aster.log");
        let mut sink = Sink::open(path.clone()).unwrap();

        let line = "x".repeat(1024) + "\n";
        while sink.written + line.len() as u64 <= MAX_BYTES {
            sink.write(&line).unwrap();
        }
        assert!(!path.with_extension("log.1").exists());

        sink.write(&line).unwrap();
        let previous = path.with_extension("log.1");
        assert!(previous.exists(), "previous log kept");
        assert!(std::fs::metadata(&previous).unwrap().len() > MAX_BYTES / 2);
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            line.len() as u64,
            "the new file holds only the last line"
        );
    }

    #[test]
    fn writing_without_init_does_not_panic() {
        write(format_args!("no sink configured"));
    }
}
