//! Opt-in, payload-free correlation of the live terminal → WebKit path.
use std::sync::OnceLock;
use std::time::{
    SystemTime,
    UNIX_EPOCH,
};

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FIG_LATENCY_TRACE").is_ok_and(|value| value == "1"))
}

pub fn record(stage: &str, session: &str, revision: Option<u64>, elapsed_ms: Option<f64>, bytes: Option<usize>) {
    if !enabled() {
        return;
    }
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0;
    let sample = serde_json::json!({ "stage": stage, "sessionId": session, "revision": revision.map(|n| n.to_string()), "unixMs": unix_ms, "milliseconds": elapsed_ms, "bytes": bytes });
    tracing::info!(target: "fig_latency", sample = %sample, "native");
}

// A diagnostic channel, never an eval or command channel. Reject arbitrary text and oversized
// batches.
pub fn webview_batch(payload: &str) {
    if !enabled() || payload.len() > 128 * 1024 {
        return;
    }
    let Ok(serde_json::Value::Array(samples)) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    if samples.len() > 512 || !samples.iter().all(valid_sample) {
        return;
    }
    tracing::info!(target: "fig_latency", samples = %serde_json::Value::Array(samples), "webview");
}
fn valid_sample(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.keys().all(|key| {
        matches!(
            key.as_str(),
            "stage"
                | "milliseconds"
                | "unixMs"
                | "clockDifferenceMs"
                | "sessionId"
                | "revision"
                | "viewEpoch"
                | "deliveryId"
                | "visible"
                | "suggestionCount"
        )
    }) && object.get("stage").and_then(|v| v.as_str()).is_some_and(|stage| {
        matches!(
            stage,
            "directory"
                | "process"
                | "process-cancelled"
                | "spec-fetch"
                | "input-received"
                | "parse"
                | "ranking"
                | "react-render"
                | "input-to-first-dom-commit"
                | "input-to-settled-dom-commit"
                | "settled-next-animation-frame"
                | "input-to-current-suggestions-dom"
                | "current-suggestions-next-frame"
        )
    }) && ["milliseconds", "unixMs", "clockDifferenceMs"]
        .iter()
        .all(|key| object.get(*key).is_some_and(|v| v.as_f64().is_some_and(f64::is_finite)))
        && ["sessionId", "revision", "viewEpoch", "deliveryId"].iter().all(|key| {
            object.get(*key).is_none_or(|v| {
                v.as_str()
                    .is_some_and(|s| s.len() <= 80 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
            })
        })
        && object.get("visible").is_none_or(|v| v.is_boolean())
        && object.get("suggestionCount").is_none_or(|v| v.as_u64().is_some())
}
pub struct Measure(&'static str, Option<std::time::Instant>);
impl Measure {
    pub fn new(stage: &'static str) -> Self {
        Self(stage, enabled().then(std::time::Instant::now))
    }
}
impl Drop for Measure {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            // Routine native lifecycle callbacks are numerous. Keep slow-handler evidence
            // without adding a disk log to every idle event-loop iteration.
            if !self.0.starts_with("loop-") || elapsed >= 0.5 {
                record(self.0, "", None, Some(elapsed), None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_samples_cannot_carry_commands_or_arbitrary_fields() {
        let mut sample = serde_json::json!({"stage":"parse", "milliseconds":2.0, "unixMs":100.0, "clockDifferenceMs":0.2, "sessionId":"abc-123", "revision":"9"});
        assert!(valid_sample(&sample));
        sample["buffer"] = "private command".into();
        assert!(!valid_sample(&sample));
        sample.as_object_mut().unwrap().remove("buffer");
        sample["stage"] = "private command".into();
        assert!(!valid_sample(&sample));
    }
}
