//! Anti-pattern #10: every backend listener binds to a loopback
//! address. PLAN.md Phase 1 step 9 requires a unit test asserting
//! this; CI fails if anything binds elsewhere.
//!
//! We don't reach into `lib::run_server_inner` directly because that
//! also wants a workspace + SQLCipher key + migrations. Instead, we
//! make two assertions:
//!   1. The literal "127.0.0.1" appears in the bind formatter in
//!      lib.rs (defense against accidental "0.0.0.0" / "::" edits).
//!   2. The Docling sidecar binds 127.0.0.1 too (defense against
//!      anyone "fixing" the Python source to listen everywhere).
//!
//! Both are static-grep tests because the alternative (actually boot
//! the server) is a noticeable build-time cost in CI for a check
//! that's really about source-code discipline.

use std::path::Path;

const BACKEND_LIB: &str = "src/lib.rs";
const DOCLING_APP: &str = "../python/docling_sidecar/app.py";

fn read_source(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn backend_binds_127_0_0_1_only() {
    let src = read_source(BACKEND_LIB);
    // The current bind formatter:
    //   let addr = format!("127.0.0.1:{port}");
    assert!(
        src.contains("\"127.0.0.1:{port}\""),
        "backend lib.rs no longer formats its bind address as 127.0.0.1; \
         double-check that nothing accidentally binds 0.0.0.0 or :: — see \
         docs/00-anti-patterns.md rule 10."
    );

    // Negative: forbid the most dangerous strings, allow comments that
    // explain why we don't bind them.
    for forbidden in ["bind(\"0.0.0.0", "bind(\"::\""] {
        assert!(
            !src.contains(forbidden),
            "backend lib.rs contains forbidden bind {forbidden}; see anti-pattern #10"
        );
    }
}

#[test]
fn docling_sidecar_binds_127_0_0_1_only() {
    let src = read_source(DOCLING_APP);
    // Sidecar uses a low-level socket bind, not Uvicorn's host arg.
    assert!(
        src.contains("s.bind((\"127.0.0.1\", 0))"),
        "docling sidecar no longer binds 127.0.0.1; see docs/05-edges.md \
         (Edge 2: Rust backend ⇄ Python sidecars are loopback-only)"
    );
    assert!(
        !src.contains("\"0.0.0.0\""),
        "docling sidecar contains a 0.0.0.0 bind"
    );
}

#[test]
fn host_validation_middleware_present() {
    let src = read_source(BACKEND_LIB);
    assert!(
        src.contains("validate_host"),
        "Host-header middleware is missing; see anti-pattern #3"
    );
    // The middleware should reject anything that isn't loopback.
    // We check the string of allowed hosts is what we expect.
    assert!(
        src.contains("\"127.0.0.1\" | \"localhost\" | \"[::1]\""),
        "host allowlist drifted; if you intentionally added a hostname \
         (e.g. for the Word add-in), update this test along with it"
    );
}
