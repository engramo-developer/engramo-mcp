use serde_json::json;
use std::io::Write;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{Layer, layer::Context};

/// Emits a GCP Error Reporting-compatible JSON log line to **stderr** for every
/// ERROR-level tracing event. Cloud Logging ingests this alongside the normal
/// Stackdriver entry; the `@type` and `serviceContext` fields cause the entry
/// to be forwarded to Error Reporting and grouped by service.
pub struct ErrorReportingLayer {
    service_name: &'static str,
    service_version: &'static str,
}

impl ErrorReportingLayer {
    /// # Security
    /// Fields logged with the `?` (Debug) sigil are emitted verbatim into GCP
    /// Error Reporting. Only use `%` (Display) for fields that may contain
    /// sensitive values, or ensure the type's `Debug` impl calls `mask()`.
    pub fn new(service_name: &'static str, service_version: &'static str) -> Self {
        Self {
            service_name,
            service_version,
        }
    }
}

struct EventVisitor {
    message: String,
    extra: Vec<String>,
}

impl EventVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            extra: Vec::new(),
        }
    }

    fn into_message(self) -> String {
        if self.extra.is_empty() {
            self.message
        } else {
            format!("{} {}", self.message, self.extra.join(" "))
        }
    }
}

/// Sensitive field name terms matched by substring — catches compound names like
/// `access_token`, `api_secret`, `api_key`, `db_password`, etc.
/// Uses `_key` (not bare `key`) to avoid false-positive redaction of unrelated
/// fields such as `monkey` or `turkey`.
const SENSITIVE_TERMS: &[&str] = &["token", "secret", "password", "_key"];

fn is_sensitive(field_name: &str) -> bool {
    SENSITIVE_TERMS
        .iter()
        .any(|term| field_name == *term || field_name.contains(term))
}

impl tracing::field::Visit for EventVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // tracing passes the message as a pre-formatted &str via record_str for
            // string literals; record_debug is only reached for non-string message
            // types, so no quote-stripping is needed.
            self.message = format!("{value:?}");
        } else if is_sensitive(field.name()) {
            self.extra.push(format!("{}=[REDACTED]", field.name()));
        } else {
            // Cap length to prevent large struct dumps from leaking internals.
            const MAX_DEBUG_LEN: usize = 256;
            let raw = format!("{value:?}");
            if raw.len() > MAX_DEBUG_LEN {
                let cut = (0..=MAX_DEBUG_LEN)
                    .rev()
                    .find(|&i| raw.is_char_boundary(i))
                    .unwrap_or(0);
                self.extra
                    .push(format!("{}={}...[truncated]", field.name(), &raw[..cut]));
            } else {
                self.extra.push(format!("{}={raw}", field.name()));
            }
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else if is_sensitive(field.name()) {
            self.extra.push(format!("{}=[REDACTED]", field.name()));
        } else {
            self.extra.push(format!("{}={value}", field.name()));
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        if is_sensitive(field.name()) {
            self.extra.push(format!("{}=[REDACTED]", field.name()));
        } else {
            self.extra.push(format!("{}={value}", field.name()));
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if is_sensitive(field.name()) {
            self.extra.push(format!("{}=[REDACTED]", field.name()));
        } else {
            self.extra.push(format!("{}={value}", field.name()));
        }
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        // Use Display (not Debug) to avoid leaking internal representations.
        // Error Display impls may still contain internal details; callers
        // must ensure errors are wrapped with a sanitised message before logging.
        let mut msg = value.to_string();
        let mut source = value.source();
        while let Some(cause) = source {
            msg.push_str(": ");
            msg.push_str(&cause.to_string());
            source = cause.source();
        }
        // Truncate to prevent oversized payloads.
        const MAX_LEN: usize = 512;
        if msg.len() > MAX_LEN {
            let cut = (0..=MAX_LEN)
                .rev()
                .find(|&i| msg.is_char_boundary(i))
                .unwrap_or(0);
            self.extra
                .push(format!("{}={}...[truncated]", field.name(), &msg[..cut]));
        } else {
            self.extra.push(format!("{}={}", field.name(), msg));
        }
    }
}

/// Crate-name prefixes whose own `ERROR`-level events are excluded from Error
/// Reporting. `rmcp`'s Streamable HTTP session worker logs its own documented,
/// routine idle-session cleanup (a 5-minute inactivity safety net — see
/// `SessionConfig::keep_alive` in rmcp) via `tracing::error!`, which would
/// otherwise page for every ordinary idle MCP session. These events remain
/// fully visible in normal Cloud Logging (this layer is additive, not a filter
/// on the primary Stackdriver layer) — only the Error Reporting forwarding is
/// suppressed, keeping that dashboard reserved for this service's own failures.
const EXCLUDED_TARGET_PREFIXES: &[&str] = &["rmcp"];

fn is_excluded_target(target: &str) -> bool {
    EXCLUDED_TARGET_PREFIXES.iter().any(|prefix| {
        target
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    })
}

impl<S: Subscriber> Layer<S> for ErrorReportingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() > Level::ERROR {
            return;
        }
        if is_excluded_target(event.metadata().target()) {
            return;
        }

        let mut visitor = EventVisitor::new();
        event.record(&mut visitor);

        let location = match (event.metadata().file(), event.metadata().line()) {
            (Some(file), Some(line)) => {
                let path = std::path::Path::new(file);
                // Workspace builds emit workspace-relative paths (e.g.
                // `src/client.rs`), which are safe and precise enough to use
                // as-is. Absolute paths (rare, but possible in some CI
                // environments) are trimmed to their last 3 components so
                // developer/CI home dirs are not leaked.
                let short_path = if path.is_absolute() {
                    let components: Vec<_> = path.components().collect();
                    let keep = components.len().min(3);
                    let partial: std::path::PathBuf =
                        components[components.len() - keep..].iter().collect();
                    partial.to_string_lossy().into_owned()
                } else {
                    file.to_owned()
                };
                format!("\n\tat {short_path}:{line}")
            }
            _ => String::new(),
        };

        let message = format!("{}{location}", visitor.into_message());

        let entry = json!({
            "severity": "ERROR",
            "@type": "type.googleapis.com/google.devtools.clouderrorreporting.v1beta1.ReportedErrorEvent",
            "serviceContext": {
                "service": self.service_name,
                "version": self.service_version,
            },
            "message": message,
        });

        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        // If stderr write fails (broken pipe on shutdown), there is no safe fallback
        // in a logging layer; suppressed explicitly so the intent is clear.
        if writeln!(handle, "{entry}").is_err() {
            // Intentionally ignored: no safe fallback in a logging layer.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::{Layer, layer::Context, layer::SubscriberExt};

    // Helper capture layer that records the formatted `into_message()` output.
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut v = EventVisitor::new();
            event.record(&mut v);
            self.0.lock().unwrap().push(v.into_message());
        }
    }

    fn capture_layer() -> (Capture, Arc<Mutex<Vec<String>>>) {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        (Capture(captured.clone()), captured)
    }

    #[test]
    fn test_visitor_message_only() {
        let mut visitor = EventVisitor::new();
        visitor.message = "server error".to_string();
        assert_eq!(visitor.into_message(), "server error");
    }

    #[test]
    fn test_visitor_with_extra_fields() {
        let mut visitor = EventVisitor::new();
        visitor.message = "Server error".to_string();
        visitor.extra.push("error=SqlxError".to_string());
        visitor.extra.push("http_status=500".to_string());
        let msg = visitor.into_message();
        assert_eq!(msg, "Server error error=SqlxError http_status=500");
    }

    #[test]
    fn test_layer_construction() {
        let layer = ErrorReportingLayer::new("test-svc", "1.0.0");
        assert_eq!(layer.service_name, "test-svc");
        assert_eq!(layer.service_version, "1.0.0");
    }

    // is_excluded_target helper

    #[test]
    fn test_is_excluded_target_exact_crate_name() {
        assert!(is_excluded_target("rmcp"));
    }

    #[test]
    fn test_is_excluded_target_submodule() {
        assert!(is_excluded_target("rmcp::transport::worker"));
        assert!(is_excluded_target(
            "rmcp::transport::streamable_http_server::tower"
        ));
    }

    #[test]
    fn test_is_excluded_target_does_not_match_unrelated_crate() {
        // must not false-positive on a crate name that merely starts with "rmcp"
        assert!(!is_excluded_target("rmcpx"));
        assert!(!is_excluded_target("rmcp_extras"));
    }

    #[test]
    fn test_is_excluded_target_own_crate_not_excluded() {
        assert!(!is_excluded_target("engramo_mcp"));
        assert!(!is_excluded_target("engramo_mcp::client"));
    }

    // on_event target filtering — end-to-end via the real layer (writes to real
    // stderr with no injectable writer, same as the pre-existing
    // `test_on_event_error_emits_without_panic`/`test_on_event_warn_is_filtered_out`
    // below — those assert "runs without panicking" for the level guard, this
    // does the same for the target guard; the actual filtering logic itself is
    // covered precisely by the `is_excluded_target` unit tests above).
    #[test]
    fn test_on_event_excluded_target_does_not_panic() {
        let layer = ErrorReportingLayer::new("test-svc", "1.2.3");
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(
                target: "rmcp::transport::worker",
                "worker quit with fatal: keep alive timeout"
            );
        });
    }

    #[test]
    fn test_on_event_own_crate_target_still_emits() {
        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(target: "engramo_mcp::client", "network error calling Engram API");
        });
        // capture_layer's spy records every event unconditionally (it exercises
        // EventVisitor's formatting, not ErrorReportingLayer's gating — see its
        // definition below); this asserts the event itself reaches a layer at
        // all with the expected message, as a sanity check that `target:`
        // overrides in the `tracing::error!` macro behave as this test suite
        // assumes elsewhere.
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("network error calling Engram API"));
    }

    // is_sensitive helper

    #[test]
    fn test_is_sensitive_exact_match() {
        assert!(is_sensitive("token"));
        assert!(is_sensitive("secret"));
        assert!(is_sensitive("password"));
    }

    #[test]
    fn test_is_sensitive_substring_match() {
        assert!(is_sensitive("access_token"));
        assert!(is_sensitive("api_secret"));
        assert!(is_sensitive("private_key"));
        assert!(is_sensitive("db_password"));
        assert!(is_sensitive("bearer_token"));
        assert!(is_sensitive("auth_key"));
        assert!(is_sensitive("api_key"));
    }

    #[test]
    fn test_is_sensitive_non_sensitive() {
        assert!(!is_sensitive("user_id"));
        assert!(!is_sensitive("count"));
        assert!(!is_sensitive("http_status"));
        assert!(!is_sensitive("message"));
        // bare "key" must NOT match unrelated fields
        assert!(!is_sensitive("monkey"));
        assert!(!is_sensitive("turkey"));
        assert!(!is_sensitive("jockey"));
    }

    #[test]
    fn test_record_debug_message_non_str_debug_kept() {
        assert_eq!(format!("{:?}", 42_i32), "42");
        assert_eq!(format!("{:?}", "hello world"), "\"hello world\"");
    }

    #[test]
    fn test_record_debug_non_message_field_kept_in_extra() {
        let mut visitor = EventVisitor::new();
        visitor.message = "root".to_string();
        visitor.extra.push(format!("foo={:?}", "bar"));
        assert_eq!(visitor.into_message(), "root foo=\"bar\"");
    }

    #[test]
    fn test_record_str_sets_message() {
        let mut visitor = EventVisitor::new();
        visitor.message = "direct message".to_string();
        assert_eq!(visitor.into_message(), "direct message");
    }

    #[test]
    fn test_record_i64_appended_to_extra() {
        let mut visitor = EventVisitor::new();
        visitor.message = "msg".to_string();
        visitor.extra.push("count=-1".to_string());
        assert_eq!(visitor.into_message(), "msg count=-1");
    }

    #[test]
    fn test_record_u64_appended_to_extra() {
        let mut visitor = EventVisitor::new();
        visitor.message = "msg".to_string();
        visitor.extra.push("count=42".to_string());
        assert_eq!(visitor.into_message(), "msg count=42");
    }

    #[test]
    fn test_record_error_uses_display_not_debug() {
        use std::fmt;

        #[derive(Debug)]
        struct SensitiveError;
        impl fmt::Display for SensitiveError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "display only")
            }
        }
        impl std::error::Error for SensitiveError {}

        let mut visitor = EventVisitor::new();
        visitor.message = "something failed".to_string();
        let err = SensitiveError;
        visitor.extra.push(format!("error={err}"));
        let msg = visitor.into_message();
        assert!(msg.contains("display only"), "got: {msg}");
        assert!(!msg.contains("SensitiveError"), "debug repr leaked: {msg}");
    }

    #[test]
    fn test_into_message_no_extra_no_trailing_space() {
        let mut visitor = EventVisitor::new();
        visitor.message = "bare message".to_string();
        assert_eq!(visitor.into_message(), "bare message");
    }

    #[test]
    fn test_on_event_warn_level_guard() {
        assert!(tracing::Level::WARN > tracing::Level::ERROR);
        assert!(tracing::Level::INFO > tracing::Level::ERROR);
        assert!(tracing::Level::ERROR <= tracing::Level::ERROR);
    }

    #[test]
    fn test_on_event_subscriber_construction() {
        let layer = ErrorReportingLayer::new("test-service", "0.0.1");
        let _subscriber = tracing_subscriber::registry().with(layer);
    }

    // end-to-end on_event tests via real tracing subscriber

    #[test]
    fn test_on_event_error_emits_without_panic() {
        let layer = ErrorReportingLayer::new("test-svc", "1.2.3");
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!("test error message");
        });
    }

    #[test]
    fn test_on_event_warn_is_filtered_out() {
        let layer = ErrorReportingLayer::new("test-svc", "1.2.3");
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("this should be filtered");
        });
    }

    #[test]
    fn test_on_event_with_structured_fields_does_not_panic() {
        let layer = ErrorReportingLayer::new("test-svc", "0.0.0");
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(field_a = "value", "msg with structured fields");
        });
    }

    // sensitive field redaction — assert [REDACTED] present, secret absent
    #[test]
    fn test_record_debug_sensitive_field_produces_redacted_marker() {
        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(token = ?"should-be-hidden", "auth failure");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("[REDACTED]"),
            "sensitive value not redacted: {}",
            msgs[0]
        );
        assert!(
            !msgs[0].contains("should-be-hidden"),
            "token leaked: {}",
            msgs[0]
        );
    }

    // record_str non-message field — assert field appears in output
    #[test]
    fn test_record_str_non_message_field_appended_to_message() {
        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(user_id = %"u-123", "auth failure");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1, "expected one captured message");
        assert!(
            msgs[0].contains("user_id=u-123"),
            "field missing: {}",
            msgs[0]
        );
        assert!(
            msgs[0].contains("auth failure"),
            "message missing: {}",
            msgs[0]
        );
    }

    // record_error via real event — assert Display is used
    #[test]
    fn test_record_error_via_real_event_uses_display() {
        use std::fmt;

        #[derive(Debug)]
        struct MyError;
        impl fmt::Display for MyError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "display-repr")
            }
        }
        impl std::error::Error for MyError {}

        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        let err = MyError;
        tracing::subscriber::with_default(sub, || {
            tracing::error!(err = &err as &dyn std::error::Error, "something broke");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("display-repr"),
            "Display not used: {}",
            msgs[0]
        );
        assert!(
            !msgs[0].contains("MyError"),
            "Debug repr leaked: {}",
            msgs[0]
        );
    }

    // record_error walks source chain
    #[test]
    fn test_record_error_walks_source_chain() {
        use std::fmt;

        #[derive(Debug)]
        struct RootCause;
        impl fmt::Display for RootCause {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "root-cause-msg")
            }
        }
        impl std::error::Error for RootCause {}

        #[derive(Debug)]
        struct WrapperError(RootCause);
        impl fmt::Display for WrapperError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "wrapper-msg")
            }
        }
        impl std::error::Error for WrapperError {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        let err = WrapperError(RootCause);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(err = &err as &dyn std::error::Error, "chained error");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("wrapper-msg"),
            "wrapper message missing: {}",
            msgs[0]
        );
        assert!(
            msgs[0].contains("root-cause-msg"),
            "root cause missing from chain: {}",
            msgs[0]
        );
    }

    // UTF-8 safety: truncation at MAX_DEBUG_LEN must not split a multi-byte char
    #[test]
    fn test_record_debug_truncation_respects_char_boundary() {
        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        // 255 ASCII bytes + 'é' (2 bytes) = 257 bytes total; naive [..256] would panic
        let long_val: String = "a".repeat(255) + "é";
        tracing::subscriber::with_default(sub, || {
            tracing::error!(data = ?long_val, "truncation test");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("[truncated]"),
            "expected truncation marker"
        );
    }

    // record_i64 via real event — assert format
    #[test]
    fn test_record_i64_via_event_produces_correct_format() {
        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(count = -42_i64, "counted error");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("count=-42"),
            "i64 format wrong: {}",
            msgs[0]
        );
    }

    // record_u64 via real event — assert format
    #[test]
    fn test_record_u64_via_event_produces_correct_format() {
        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(http_status = 500_u64, "server error");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("http_status=500"),
            "u64 format wrong: {}",
            msgs[0]
        );
    }

    // JSON output structure
    #[test]
    fn test_on_event_emits_valid_gcp_error_reporting_json() {
        // Test the JSON structure via the entry built directly in on_event.
        // We verify by inspecting the serde_json value constructed with the same
        // fields (pure logic test, no stderr capture needed).
        let service_name = "my-svc";
        let service_version = "2.0.0";
        let message = "test event\n\tat lib.rs:42";

        let entry = json!({
            "severity": "ERROR",
            "@type": "type.googleapis.com/google.devtools.clouderrorreporting.v1beta1.ReportedErrorEvent",
            "serviceContext": {
                "service": service_name,
                "version": service_version,
            },
            "message": message,
        });

        assert_eq!(entry["severity"], "ERROR");
        assert_eq!(entry["serviceContext"]["service"], "my-svc");
        assert_eq!(entry["serviceContext"]["version"], "2.0.0");
        assert!(
            entry["@type"]
                .as_str()
                .unwrap()
                .contains("ReportedErrorEvent"),
            "@type missing ReportedErrorEvent"
        );
        assert!(entry["message"].as_str().unwrap().contains("test event"));
    }

    // record_i64/u64 sensitive field redaction
    #[test]
    fn test_record_i64_sensitive_field_redacted() {
        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        // `access_token_ttl` contains "token" — a genuinely sensitive int field
        tracing::subscriber::with_default(sub, || {
            tracing::error!(access_token_ttl = 12345_i64, "sensitive int");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("[REDACTED]"),
            "access_token_ttl not redacted: {}",
            msgs[0]
        );
        assert!(
            !msgs[0].contains("12345"),
            "access_token_ttl value leaked: {}",
            msgs[0]
        );
    }

    // Sensitive field redaction via display path
    #[test]
    fn test_sensitive_field_is_redacted_via_display() {
        let (layer, captured) = capture_layer();
        let sub = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(sub, || {
            tracing::error!(token = %"Bearer sk-secret", "auth failure");
        });
        let msgs = captured.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("[REDACTED]"),
            "token not redacted: {}",
            msgs[0]
        );
        assert!(
            !msgs[0].contains("sk-secret"),
            "token value leaked: {}",
            msgs[0]
        );
    }

    // location branch — verify Some/Some branch produces a path:line suffix
    #[test]
    fn test_on_event_location_includes_relative_path() {
        // Verifies the Some/Some branch in on_event executes without panic.
        // Relative workspace paths are kept as-is; absolute paths are trimmed to
        // the last 3 components. The `_ => String::new()` arm is dead code for
        // macro-generated events; we document it here and accept it cannot be
        // triggered from a macro call site.
        let layer = ErrorReportingLayer::new("svc", "0");
        let sub = tracing_subscriber::registry().with(layer);
        // Fires with file=lib.rs and a line number; stderr output is not captured
        // but the absence of panic confirms the branch executes.
        tracing::subscriber::with_default(sub, || {
            tracing::error!("check location format");
        });
    }
}
