/**
 * Env-var allowlist for spawned children (backend, sidecars).
 *
 * Anti-pattern #9 (docs/00-anti-patterns.md): Electron doesn't leak
 * its own env (which inherits from the user's shell, possibly
 * including ANTHROPIC_API_KEY etc. set there) to its children. Each
 * child gets ONLY the names whitelisted here, plus the ones the
 * caller explicitly sets afterward.
 *
 * The set is intentionally small. If a child needs additional names:
 *   - For supervisor / runtime metadata, pass via Mike-namespaced
 *     env (MIKE_BACKEND_*, MIKE_SIDECAR_*).
 *   - For OS-level needs (locale, temp dirs, PATH), add to the
 *     allowlist below with a one-line justification.
 */

const ALLOWLIST = [
  // Required for finding executables (Python, sub-binaries).
  "PATH",
  // Some Python packages assert HOME exists.
  "HOME",
  // Temp dirs — stdlib + most libraries fall back to these.
  "TMPDIR",
  "TMP",
  "TEMP",
  // Locale — some Python libs print or parse with the system locale.
  "LANG",
  "LC_ALL",
  // Rust-side log filter (`tracing-subscriber`'s EnvFilter).
  "RUST_LOG",
] as const;

export function safeChildEnv(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const key of ALLOWLIST) {
    const v = process.env[key];
    if (v !== undefined) env[key] = v;
  }
  return env;
}

/**
 * For sidecar spawn — same allowlist plus a few names that Python
 * environments commonly need to find their site-packages.
 */
export function safeSidecarEnv(): NodeJS.ProcessEnv {
  const env = safeChildEnv();
  for (const key of ["PYTHONPATH", "VIRTUAL_ENV"] as const) {
    const v = process.env[key];
    if (v !== undefined) env[key] = v;
  }
  return env;
}
