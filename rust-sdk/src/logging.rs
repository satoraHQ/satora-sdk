//! Structured SDK events for an application logger, with the TypeScript SDK's
//! event names, levels and secret redaction.
//!
//! Without a custom logger, events go to `tracing` under the `lendaswap_sdk`
//! target and the subscriber filters them. A custom logger set with
//! [`crate::ClientBuilder::logger`] replaces that sink and is silent until a
//! level is set, as in TypeScript.

use serde_json::Map;
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;

/// Minimum severity a client emits. `Silent` emits nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    #[default]
    Silent,
}

impl FromStr for LogLevel {
    type Err = ParseLogLevelError;

    /// Parses the TypeScript level names (`"trace"` to `"silent"`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            "silent" => Ok(Self::Silent),
            other => Err(ParseLogLevelError(other.to_owned())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown log level {0:?}; expected trace, debug, info, warn, error or silent")]
pub struct ParseLogLevelError(String);

/// One SDK event. `level` is never `Silent`. Amounts in `data` are decimal
/// strings and keys are camelCase, as in TypeScript.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LogRecord {
    pub level: LogLevel,
    pub message: String,
    pub event: Option<String>,
    pub module: Option<String>,
    pub operation: Option<String>,
    pub swap_id: Option<String>,
    /// Context and event fields, with secret-named keys already redacted.
    pub data: Map<String, Value>,
    /// Display string of the error that caused the event.
    pub error: Option<String>,
}

/// Receives SDK events. It is called synchronously, in some flows while an
/// execution lock is held, so it should return quickly, and it must not panic.
pub trait Logger: Send + Sync {
    fn log(&self, record: &LogRecord);
    /// Lets a sink skip building records it would drop.
    fn enabled(&self, _level: LogLevel) -> bool {
        true
    }
}

impl<F: Fn(&LogRecord) + Send + Sync> Logger for F {
    fn log(&self, record: &LogRecord) {
        self(record)
    }
}

/// Forwards events to `tracing` under the `lendaswap_sdk` target, with the
/// record's fields as `event`, `module`, `operation`, `swap_id`, `data` (JSON)
/// and `error`. This is the sink when no logger is set. With no subscriber,
/// events reach a `log` crate logger through tracing's `log` fallback.
#[derive(Clone, Copy, Debug, Default)]
pub struct TracingLogger;

struct Json<'a>(&'a Map<String, Value>);
impl std::fmt::Display for Json<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&serde_json::to_string(self.0).map_err(|_| std::fmt::Error)?)
    }
}

macro_rules! tracing_record {
    ($macro:ident, $record:expr) => {
        tracing::$macro!(
            target: "lendaswap_sdk",
            event = $record.event.as_deref(),
            module = $record.module.as_deref(),
            operation = $record.operation.as_deref(),
            swap_id = $record.swap_id.as_deref(),
            data = (!$record.data.is_empty()).then(|| tracing::field::display(Json(&$record.data))),
            error = $record.error.as_deref(),
            "{}",
            $record.message
        )
    };
}

// No `enabled` override: `tracing::enabled!` has no `log` fallback, so asking it
// would drop every event for an app that logs through `log` without a subscriber.
// The event macros filter by themselves and keep that fallback.
impl Logger for TracingLogger {
    fn log(&self, record: &LogRecord) {
        match record.level {
            LogLevel::Trace => tracing_record!(trace, record),
            LogLevel::Debug => tracing_record!(debug, record),
            LogLevel::Info => tracing_record!(info, record),
            LogLevel::Warn => tracing_record!(warn, record),
            LogLevel::Error => tracing_record!(error, record),
            LogLevel::Silent => {}
        }
    }
}

/// Drops every event.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopLogger;

impl Logger for NoopLogger {
    fn log(&self, _record: &LogRecord) {}

    fn enabled(&self, _level: LogLevel) -> bool {
        false
    }
}

const SECRET_NAMES: [&str; 12] = [
    "secret",
    "secretkey",
    "usersecretkey",
    "privatekey",
    "mnemonic",
    "xprv",
    "preimage",
    "signature",
    "authorization",
    "apikey",
    "api_key",
    "bearer",
];
const REDACTED: &str = "[REDACTED]";

/// Replace the values of secret-named keys with `"[REDACTED]"`, recursing into
/// objects and arrays. Names match as TypeScript's `redactLogValue` does: case
/// insensitive and delimited by anything but a letter or digit, so `preimage`,
/// `userSecretKey` and `api_key` are redacted while `preimageHash` is not.
pub fn redact_log_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(redact_log_value).collect()),
        Value::Object(map) => Value::Object(redact_map(map)),
        value @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)) => value,
    }
}

fn redact_map(map: Map<String, Value>) -> Map<String, Value> {
    map.into_iter()
        .map(|(key, value)| {
            let value = if is_secret_key(&key) {
                Value::String(REDACTED.into())
            } else {
                redact_log_value(value)
            };
            (key, value)
        })
        .collect()
}

// TypeScript's pattern is `(^|_|-|\b)(names)(_|-|\b|$)` with the `i` flag. Every
// name starts and ends with a word character, so a `\b` there means the
// neighbour is absent or not in [A-Za-z0-9_], and `_` is allowed explicitly.
fn is_secret_key(key: &str) -> bool {
    let key = key.as_bytes();
    let delimiter = |byte: Option<&u8>| byte.is_none_or(|b| !b.is_ascii_alphanumeric());
    SECRET_NAMES.iter().any(|name| {
        key.windows(name.len()).enumerate().any(|(start, window)| {
            window.eq_ignore_ascii_case(name.as_bytes())
                && delimiter(start.checked_sub(1).and_then(|i| key.get(i)))
                && delimiter(key.get(start + name.len()))
        })
    })
}

#[derive(Clone, Default)]
struct LogContext {
    module: Option<&'static str>,
    operation: Option<&'static str>,
    swap_id: Option<String>,
    data: Map<String, Value>,
}

/// A client's sink and level with the context of the current operation.
#[derive(Clone)]
pub(crate) struct SdkLogger {
    sink: Arc<dyn Logger>,
    level: LogLevel,
    context: Arc<LogContext>,
}

impl SdkLogger {
    pub(crate) fn new(sink: Arc<dyn Logger>, level: LogLevel) -> Self {
        Self {
            sink,
            level,
            context: Arc::default(),
        }
    }

    /// The default: everything to `tracing`, filtered by the subscriber.
    pub(crate) fn tracing() -> Self {
        Self::new(Arc::new(TracingLogger), LogLevel::Trace)
    }

    /// Records from the child carry this module and operation.
    pub(crate) fn child(&self, module: &'static str, operation: &'static str) -> Self {
        let mut child = self.clone();
        if child.level != LogLevel::Silent {
            let context = Arc::make_mut(&mut child.context);
            context.module = Some(module);
            context.operation = Some(operation);
        }
        child
    }

    pub(crate) fn swap_id(mut self, id: &str) -> Self {
        if self.level != LogLevel::Silent {
            Arc::make_mut(&mut self.context).swap_id = Some(id.to_owned());
        }
        self
    }

    /// Adds a context field to every record; an event's own field of the same
    /// name wins.
    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        if self.level != LogLevel::Silent {
            Arc::make_mut(&mut self.context)
                .data
                .insert(key.to_owned(), value.into());
        }
        self
    }

    pub(crate) fn enabled(&self, level: LogLevel) -> bool {
        level != LogLevel::Silent && level >= self.level && self.sink.enabled(level)
    }

    /// Use `sdk_log!`, which checks `enabled` before building `data`.
    pub(crate) fn emit(
        &self,
        level: LogLevel,
        event: &'static str,
        message: &'static str,
        data: Map<String, Value>,
        error: Option<String>,
    ) {
        let mut merged = self.context.data.clone();
        merged.extend(data);
        self.sink.log(&LogRecord {
            level,
            message: message.to_owned(),
            event: Some(event.to_owned()),
            module: self.context.module.map(str::to_owned),
            operation: self.context.operation.map(str::to_owned),
            swap_id: self.context.swap_id.clone(),
            data: redact_map(merged),
            error,
        });
    }
}

/// `sdk_log!(log, Info, "event.name", "Message", "key" => value, ...; error = err)`.
/// Event and message are `&'static str`. Values go through `serde_json::json!`
/// and are only evaluated when the level is enabled, so they must not have side
/// effects; `error` takes anything with `Display`.
macro_rules! sdk_log {
    (@emit $log:expr, $level:ident, $event:expr, $message:expr, $error:expr $(, $key:literal => $value:expr)*) => {{
        let log = &$log;
        let level = $crate::logging::LogLevel::$level;
        if log.enabled(level) {
            #[allow(unused_mut)]
            let mut data = ::serde_json::Map::new();
            $(data.insert(::std::string::String::from($key), ::serde_json::json!($value));)*
            log.emit(level, $event, $message, data, $error);
        }
    }};
    ($log:expr, $level:ident, $event:expr, $message:expr $(, $key:literal => $value:expr)* ; error = $error:expr) => {
        $crate::logging::sdk_log!(@emit $log, $level, $event, $message, Some(($error).to_string()) $(, $key => $value)*)
    };
    ($log:expr, $level:ident, $event:expr, $message:expr $(, $key:literal => $value:expr)* $(,)?) => {
        $crate::logging::sdk_log!(@emit $log, $level, $event, $message, None $(, $key => $value)*)
    };
}
pub(crate) use sdk_log;

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingLogger(std::sync::Mutex<Vec<LogRecord>>);

#[cfg(test)]
impl RecordingLogger {
    pub(crate) fn records(&self) -> Vec<LogRecord> {
        self.0.lock().unwrap().clone()
    }

    pub(crate) fn events(&self) -> Vec<String> {
        self.records()
            .into_iter()
            .filter_map(|record| record.event)
            .collect()
    }

    pub(crate) fn find(&self, event: &str) -> Option<LogRecord> {
        self.records()
            .into_iter()
            .find(|record| record.event.as_deref() == Some(event))
    }
}

#[cfg(test)]
impl Logger for RecordingLogger {
    fn log(&self, record: &LogRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    fn recording(level: LogLevel) -> (SdkLogger, Arc<RecordingLogger>) {
        let sink = Arc::new(RecordingLogger::default());
        (SdkLogger::new(sink.clone(), level), sink)
    }

    #[test]
    fn a_logger_without_a_level_is_silent() {
        let sink = Arc::new(RecordingLogger::default());
        let client = crate::Client::builder()
            .mnemonic(
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            )
            .logger(sink.clone())
            .build()
            .unwrap();
        sdk_log!(client.log, Error, "e", "hello");
        assert!(sink.records().is_empty());
        let (log, sink) = recording(LogLevel::Silent);
        sdk_log!(log.child("m", "o"), Error, "e", "hello");
        assert!(sink.records().is_empty());
    }

    #[test]
    fn levels_below_the_configured_one_are_dropped() {
        let (log, sink) = recording(LogLevel::Warn);
        sdk_log!(log, Info, "ignored", "ignored");
        sdk_log!(log, Warn, "emitted", "emitted");
        sdk_log!(log, Error, "error", "error");
        assert_eq!(sink.events(), ["emitted", "error"]);
        assert_eq!(sink.records()[0].level, LogLevel::Warn);
    }

    #[test]
    fn values_are_not_built_for_disabled_levels() {
        let (log, _) = recording(LogLevel::Info);
        let built = std::cell::Cell::new(false);
        let value = || {
            built.set(true);
            1
        };
        sdk_log!(log, Debug, "e", "m", "key" => value());
        assert!(!built.get());
    }

    #[test]
    fn redacts_secret_fields_like_typescript() {
        assert_eq!(
            redact_log_value(json!({
                "userSecretKey": "secret",
                "nested": {"preimage": "secret", "txid": "public"},
            })),
            json!({
                "userSecretKey": "[REDACTED]",
                "nested": {"preimage": "[REDACTED]", "txid": "public"},
            })
        );
        assert_eq!(
            redact_log_value(json!({
                "preimageHash": "kept",
                "api_key": "x",
                "X-API-KEY": "kept",
                "secretive_secret": "x",
                "secreté": "x",
                "authorization": "x",
                "items": [{"Signature": "x", "signatures": "kept"}, "plain"],
                "preimage_hash": "x",
                "my.mnemonic": "x",
                "xprvs": "kept",
                "bearer_token": "x",
                "secretive": "kept",
                "top_secret": "x",
            })),
            json!({
                "preimageHash": "kept",
                "api_key": "[REDACTED]",
                "X-API-KEY": "kept",
                "secretive_secret": "[REDACTED]",
                "secreté": "[REDACTED]",
                "authorization": "[REDACTED]",
                "items": [{"Signature": "[REDACTED]", "signatures": "kept"}, "plain"],
                "preimage_hash": "[REDACTED]",
                "my.mnemonic": "[REDACTED]",
                "xprvs": "kept",
                "bearer_token": "[REDACTED]",
                "secretive": "kept",
                "top_secret": "[REDACTED]",
            })
        );
    }

    #[test]
    fn children_override_context_and_merge_data_with_the_event_winning() {
        let (log, sink) = recording(LogLevel::Trace);
        let parent = log
            .child("a", "a.op")
            .swap_id("s1")
            .with("x", 1)
            .with("y", 1);
        let child = parent.child("b", "b.op").with("y", 2).with("secret", "s");
        sdk_log!(child, Info, "b.event", "message", "y" => 3, "z" => "zz"; error = "boom");
        sdk_log!(parent, Debug, "a.event", "message");
        let records = sink.records();
        assert_eq!(
            records[0],
            LogRecord {
                level: LogLevel::Info,
                message: "message".into(),
                event: Some("b.event".into()),
                module: Some("b".into()),
                operation: Some("b.op".into()),
                swap_id: Some("s1".into()),
                data: json!({"x": 1, "y": 3, "z": "zz", "secret": "[REDACTED]"})
                    .as_object()
                    .unwrap()
                    .clone(),
                error: Some("boom".into()),
            }
        );
        assert_eq!(records[1].module.as_deref(), Some("a"));
        assert_eq!(
            records[1].data,
            *json!({"x": 1, "y": 1}).as_object().unwrap()
        );
    }

    #[test]
    fn closures_are_loggers_and_levels_parse_from_typescript_names() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let log = SdkLogger::new(
            Arc::new(move |record: &LogRecord| sink.lock().unwrap().push(record.clone())),
            "debug".parse().unwrap(),
        );
        sdk_log!(log, Trace, "dropped", "m");
        sdk_log!(log, Debug, "kept", "m");
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!("verbose".parse::<LogLevel>().is_err());
        assert_eq!("silent".parse::<LogLevel>(), Ok(LogLevel::Silent));
        assert!(!SdkLogger::new(Arc::new(NoopLogger), LogLevel::Trace).enabled(LogLevel::Error));
    }

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
        type Writer = Buffer;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn clients_without_a_logger_deliver_through_a_tracing_subscriber_filter() {
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let built = crate::Client::builder().mnemonic(mnemonic).build().unwrap();
        let warn_only = crate::Client::builder()
            .mnemonic(mnemonic)
            .log_level(LogLevel::Warn)
            .build()
            .unwrap();
        let plain = crate::Client::new("http://localhost:3333").unwrap();
        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .with_env_filter("lendaswap_sdk=debug")
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let log = built
                .log
                .child("redeem/arkade", "arkade.claim")
                .swap_id("swap-1");
            sdk_log!(log, Trace, "arkade.claim.hidden", "Not shown");
            sdk_log!(log, Info, "arkade.claim.start", "Starting Arkade claim", "preimage" => "00", "vtxoCount" => 2);
            sdk_log!(plain.log, Info, "plain", "From Client::new");
            sdk_log!(warn_only.log, Info, "below", "Below the client level");
            sdk_log!(warn_only.log, Warn, "at", "At the client level");
        });
        let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        assert!(!output.contains("Not shown"), "{output}");
        assert!(!output.contains("Below the client level"), "{output}");
        assert!(output.contains("From Client::new"), "{output}");
        assert!(output.contains("At the client level"), "{output}");
        let line = output
            .lines()
            .find(|line| line.contains("Starting Arkade claim"))
            .unwrap_or_else(|| panic!("{output}"));
        for expected in [
            "INFO",
            "lendaswap_sdk",
            "arkade.claim.start",
            "redeem/arkade",
            "swap-1",
            r#"{"preimage":"[REDACTED]","vtxoCount":2}"#,
        ] {
            assert!(line.contains(expected), "{expected} missing from {line}");
        }
    }
}
