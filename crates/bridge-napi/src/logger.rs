//! Routes Rust `log` records to a JS logger. The TS façade adapts the pino-style
//! `ILogger` into a flat `(level, target, msg) => void` dispatch; here we wrap it
//! in a **weak, non-blocking** ThreadsafeFunction (logging must never block a
//! worker thread nor keep the process alive). `log::set_max_level` (from the JS
//! logger's level) is the single source of truth for filtering.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use log::{LevelFilter, Log, Metadata, Record};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};

/// `(level: u32, target: String, msg: String) => void`, weak (won't pin the loop).
/// `level` is the `log::Level` discriminant: Error=1, Warn=2, Info=3, Debug=4, Trace=5.
pub type LogTsfn = ThreadsafeFunction<
    FnArgs<(u32, String, String)>,
    (),
    FnArgs<(u32, String, String)>,
    Status,
    false,
    true,
>;

static LOG_TSFN: RwLock<Option<Arc<LogTsfn>>> = RwLock::new(None);
static LOGGER_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Pino-style level parsing (string names + numeric aliases), ported verbatim.
pub fn parse_level_filter(level_str: &str) -> LevelFilter {
    match level_str.to_lowercase().as_str() {
        "trace" | "10" => LevelFilter::Trace,
        "debug" | "20" => LevelFilter::Debug,
        "info" | "30" => LevelFilter::Info,
        "warn" | "warning" | "40" => LevelFilter::Warn,
        "error" | "50" | "60" => LevelFilter::Error,
        "silent" | "off" => LevelFilter::Off,
        _ => LevelFilter::Info,
    }
}

struct BridgeLogger;

impl Log for BridgeLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        // `log::set_max_level` short-circuits below-threshold records before we
        // are called, so every record that reaches us is enabled.
        true
    }

    fn log(&self, record: &Record) {
        let guard = LOG_TSFN.read().unwrap_or_else(|e| e.into_inner());
        if let Some(tsfn) = guard.as_ref() {
            let level = record.level() as u32;
            let target = record.target().to_string();
            let msg = record.args().to_string();
            tsfn.call(
                FnArgs::from((level, target, msg)),
                ThreadsafeFunctionCallMode::NonBlocking,
            );
        }
    }

    fn flush(&self) {}
}

static BRIDGE_LOGGER: BridgeLogger = BridgeLogger;

/// Install (or swap) the JS log sink and set the max level. `log::set_logger` is
/// process-global and set once; subsequent calls only swap the stored TSFN.
pub fn install(tsfn: Arc<LogTsfn>, level: LevelFilter) {
    *LOG_TSFN.write().unwrap_or_else(|e| e.into_inner()) = Some(tsfn);
    log::set_max_level(level);
    if !LOGGER_INSTALLED.swap(true, Ordering::SeqCst) {
        let _ = log::set_logger(&BRIDGE_LOGGER);
    }
}
