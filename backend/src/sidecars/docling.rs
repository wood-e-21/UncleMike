//! Docling — PDF/DOCX parsing sidecar.
//!
//! Phase 1: Electron spawns the Python process and the supervisor
//! here is read-only (probe `/health` + `/version`, surface state).
//! See `electron/docling.ts`.
//!
//! Phase 3: this module will gain the spawn logic.

use std::collections::HashMap;

use super::{Sidecar, SidecarConcurrency};

pub struct Docling;

#[async_trait::async_trait]
impl Sidecar for Docling {
    fn name(&self) -> &'static str {
        "docling"
    }

    fn expected_major_version(&self) -> u32 {
        // Bump this when the wire format changes (request/response
        // schemas in `python/docling_sidecar/app.py`). Mismatches
        // surface as `degraded: version-mismatch` per docs/03.
        1
    }

    fn concurrency(&self) -> SidecarConcurrency {
        SidecarConcurrency::MultiWorker { default: 2, max: 4 }
    }

    fn extra_env(&self) -> HashMap<String, String> {
        // Docling-specific runtime config. The supervisor (Phase 3)
        // will merge this with the universal MIKE_SIDECAR_* envelope.
        // Today these are set directly by `electron/docling.ts`.
        let mut env = HashMap::new();
        // Devices the user can pick; defaults set by Electron based
        // on the host OS. Listed here so the trait knows the surface.
        env.insert("MIKE_DOCLING_DEVICE".into(), String::new());
        env.insert("MIKE_DOCLING_MAX_TOKENS".into(), String::new());
        env
    }
}
