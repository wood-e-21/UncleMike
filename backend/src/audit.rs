//! Append-only audit log. Phase-1 stub: writes JSONL events to
//! `<workspace>/.mike/logs/audit.log`. No rotation, no buffering;
//! Phase 5 (per PLAN.md) will add 50MB rotation and the full event
//! catalogue.
//!
//! Why a stub now: domain routes need a place to call
//! `audit::log(event)` from. Adding the call sites today (auth
//! login/logout in particular) means Phase 5 just turns up the
//! sophistication of the writer; we don't have to find every place
//! that should have logged.

use chrono::Utc;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::workspace::WorkspacePaths;

/// Log file lock — coarse-grained on purpose. Audit events should be
/// rare relative to API traffic; a single mutex is fine for v1.
/// Phase 5 will replace with a tokio::sync::Mutex + bounded channel.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// What to record. Each variant has a stable name in the JSONL output
/// (the serialize-tag is the kind). Add variants as new domain events
/// land; never remove or rename existing ones (the audit log is
/// append-only and consumers grep by kind).
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditEvent<'a> {
    AuthLogin { user_id: &'a str },
    AuthLogout { user_id: &'a str },
    SecretsLoaded { populated: usize },
    SecretsCleared,
    WorkspaceOpened,
    WorkspaceClosed,
    /// Anything we don't yet have a typed variant for. Use sparingly;
    /// the goal is for greppable kinds to dominate. Field is named
    /// `subkind` (not `kind`) to avoid colliding with the serde tag.
    Generic { subkind: &'a str, detail: &'a str },
}

#[derive(Debug, Serialize)]
struct AuditEntry<'a> {
    /// RFC 3339 UTC timestamp. Kept first in the JSON so a `head -1`
    /// of the file shows the start time.
    ts: String,
    #[serde(flatten)]
    event: AuditEvent<'a>,
}

fn audit_log_path(paths: &WorkspacePaths) -> PathBuf {
    paths.mike_dir.join("logs").join("audit.log")
}

/// Append one event. Failure to write is logged via tracing but does
/// NOT propagate — audit failures should never break the user's
/// request. Phase 5 will add a counter so silent failures surface in
/// a /system/status field.
pub fn log(paths: &WorkspacePaths, event: AuditEvent<'_>) {
    let entry = AuditEntry {
        ts: Utc::now().to_rfc3339(),
        event,
    };
    let line = match serde_json::to_string(&entry) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("[audit] serialize failed: {e}");
            return;
        }
    };
    let path = audit_log_path(paths);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!("[audit] mkdir {} failed: {e}", parent.display());
            return;
        }
    }
    let _guard = WRITE_LOCK.lock().expect("audit lock poisoned");
    let res = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);
    match res {
        Ok(mut f) => {
            use std::io::Write;
            if let Err(e) = writeln!(f, "{line}") {
                tracing::warn!("[audit] write failed: {e}");
            }
        }
        Err(e) => tracing::warn!("[audit] open {} failed: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::WorkspacePaths;

    #[test]
    fn writes_one_jsonl_per_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = WorkspacePaths::new(dir.path()).expect("paths");
        log(&paths, AuditEvent::WorkspaceOpened);
        log(
            &paths,
            AuditEvent::AuthLogin {
                user_id: "test-user",
            },
        );
        let contents = std::fs::read_to_string(audit_log_path(&paths)).expect("read log");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "two events → two lines");
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
            assert!(v.get("ts").is_some(), "ts present");
            assert!(v.get("kind").is_some(), "kind present");
        }
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["kind"], "workspace_opened");
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["kind"], "auth_login");
        assert_eq!(second["user_id"], "test-user");
    }
}
