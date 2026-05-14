//! Supervisor. Phase 1 implementation: reads the runtime file
//! Electron wrote, runs health + version probes, caches the result
//! with a short TTL.
//!
//! Phase 3 implementation (future): owns `tokio::process::Command`
//! spawn, restart-with-backoff, SIGTERM-then-SIGKILL on shutdown.
//! The public surface of this module is the same in both phases, so
//! callers (routes, the /system/status handler) don't change.

use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::sidecars::{Sidecar, SupervisorState};

const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const STATE_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
struct RuntimeFile {
    port: u16,
    #[serde(default)]
    #[allow(dead_code)]
    pid: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct VersionResponse {
    version: String,
    #[serde(default)]
    #[allow(dead_code)]
    schema_version: Option<u32>,
    #[serde(default)]
    #[allow(dead_code)]
    capabilities: Vec<String>,
}

struct Slot {
    sidecar: Arc<dyn Sidecar>,
    cached: RwLock<Option<(Instant, SupervisorState)>>,
}

/// One supervisor for the whole process. Holds one slot per
/// registered sidecar. Today: only docling.
pub struct Supervisor {
    slots: Vec<Slot>,
    workspace_root: PathBuf,
    http: reqwest::Client,
}

impl Supervisor {
    pub fn new(workspace_root: PathBuf, sidecars: Vec<Arc<dyn Sidecar>>) -> Self {
        let slots = sidecars
            .into_iter()
            .map(|s| Slot {
                sidecar: s,
                cached: RwLock::new(None),
            })
            .collect();
        Self {
            slots,
            workspace_root,
            http: reqwest::Client::builder()
                .timeout(HEALTH_PROBE_TIMEOUT)
                .build()
                .expect("reqwest client"),
        }
    }

    fn runtime_path(&self, name: &str) -> PathBuf {
        // Per docs/01-workspace-layout.md:
        //   <workspace>/.mike/runtime/sidecars/<name>.json
        self.workspace_root
            .join(".mike")
            .join("runtime")
            .join("sidecars")
            .join(format!("{name}.json"))
    }

    /// Look up a registered sidecar by name. Returns None if not
    /// registered (i.e. the supervisor was constructed without it —
    /// typically because the feature is off, not because there's a
    /// bug).
    fn slot(&self, name: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.sidecar.name() == name)
    }

    /// Probe a sidecar's state. Cached for STATE_CACHE_TTL because
    /// every /system/status hit would otherwise round-trip two HTTP
    /// calls per sidecar.
    pub async fn state(&self, name: &str) -> SupervisorState {
        let Some(slot) = self.slot(name) else {
            return SupervisorState::Down;
        };
        if let Some((at, state)) = slot.cached.read().await.clone() {
            if at.elapsed() < STATE_CACHE_TTL {
                return state;
            }
        }
        let fresh = self.probe(slot).await;
        *slot.cached.write().await = Some((Instant::now(), fresh.clone()));
        fresh
    }

    /// Invalidate the cache for one sidecar — call this when the
    /// supervisor knows the state changed (e.g. after a restart in
    /// Phase 3).
    #[allow(dead_code)]
    pub async fn invalidate(&self, name: &str) {
        if let Some(slot) = self.slot(name) {
            *slot.cached.write().await = None;
        }
    }

    async fn probe(&self, slot: &Slot) -> SupervisorState {
        let name = slot.sidecar.name();
        let path = self.runtime_path(name);
        let Ok(bytes) = std::fs::read(&path) else {
            tracing::debug!("[sidecar:{name}] runtime file missing at {}", path.display());
            return SupervisorState::Down;
        };
        let rt: RuntimeFile = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                return SupervisorState::Degraded {
                    reason: format!("runtime file unparseable: {e}"),
                };
            }
        };

        let base = format!("http://127.0.0.1:{}", rt.port);

        // /health is mandatory.
        let health_url = format!("{base}/health");
        match self.http.get(&health_url).send().await {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => {
                return SupervisorState::Degraded {
                    reason: format!("/health returned {}", r.status()),
                };
            }
            Err(e) => {
                return SupervisorState::Degraded {
                    reason: format!("/health unreachable: {e}"),
                };
            }
        }

        // /version is required per docs/03-sidecars.md. Absent or
        // unparseable → degraded. A sidecar that's "healthy" but
        // version-unknown is not safe to call (might be the wrong
        // major).
        let version_url = format!("{base}/version");
        let version_resp = match self.http.get(&version_url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                return SupervisorState::Degraded {
                    reason: format!("/version returned {}; sidecar pre-dates docs/03-sidecars.md", r.status()),
                };
            }
            Err(e) => {
                return SupervisorState::Degraded {
                    reason: format!("/version unreachable: {e}"),
                };
            }
        };
        let version: VersionResponse = match version_resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return SupervisorState::Degraded {
                    reason: format!("/version body not JSON: {e}"),
                };
            }
        };

        // Compare major versions. A sidecar that's a major behind /
        // ahead is degraded — we refuse to call it rather than
        // silently emit wrong-shaped requests.
        let major = parse_major(&version.version);
        let expected = slot.sidecar.expected_major_version();
        if let Some(maj) = major {
            if maj != expected {
                return SupervisorState::Degraded {
                    reason: format!(
                        "version major {maj} != expected {expected}; update Mike or the sidecar"
                    ),
                };
            }
        } else {
            return SupervisorState::Degraded {
                reason: format!(
                    "could not parse version major from {:?}; expected semver",
                    version.version
                ),
            };
        }

        SupervisorState::Healthy {
            port: rt.port,
            version: version.version,
        }
    }
}

fn parse_major(version: &str) -> Option<u32> {
    let head = version.split('.').next()?;
    // Strip optional `v` prefix (`v1.2.3`).
    let head = head.strip_prefix('v').unwrap_or(head);
    head.parse().ok()
}

/// Construct the supervisor with the default sidecars (today: just
/// Docling). Phase 3 will pass in additional sidecars (eyecite).
pub fn build_default(workspace_root: PathBuf) -> Result<Arc<Supervisor>> {
    let sidecars: Vec<Arc<dyn Sidecar>> = vec![Arc::new(super::docling::Docling)];
    Ok(Arc::new(Supervisor::new(workspace_root, sidecars)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_major_handles_common_forms() {
        assert_eq!(parse_major("1.0.0"), Some(1));
        assert_eq!(parse_major("v2.3.4"), Some(2));
        assert_eq!(parse_major("0.5.6"), Some(0));
        assert_eq!(parse_major("not-a-version"), None);
    }
}
