#!/usr/bin/env node
/* eslint-disable no-console */
/**
 * Smoke-test the backend exactly as the packaged Electron app starts it:
 * through Mike.app's executable with ELECTRON_RUN_AS_NODE=1.
 *
 * This catches the highest-risk packaging failure: native modules compiled
 * for the wrong Node/Electron ABI.
 */
const { spawn } = require("child_process");
const fs = require("fs");
const http = require("http");
const path = require("path");

const ROOT = path.resolve(__dirname, "..");
const APP = path.join(ROOT, "dist", "mac-arm64", "Mike.app");
const BIN = path.join(APP, "Contents", "MacOS", "Mike");
const RESOURCES = path.join(APP, "Contents", "Resources");
const BACKEND = path.join(RESOURCES, "backend");
const ENTRY = path.join(BACKEND, "dist", "index.js");
const WORKSPACE =
  process.env.MIKE_SMOKE_WORKSPACE ||
  path.join("/private/tmp", "mike-packaged-backend-smoke");
const RUNTIME = path.join(WORKSPACE, ".mike", "runtime.json");

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function health(port) {
  return new Promise((resolve, reject) => {
    const req = http.get(
      {
        hostname: "127.0.0.1",
        port,
        path: "/health",
        timeout: 1000,
      },
      (res) => {
        res.resume();
        resolve(res.statusCode === 200);
      },
    );
    req.on("timeout", () => {
      req.destroy(new Error("health request timed out"));
    });
    req.on("error", reject);
  });
}

function readRuntimePort() {
  try {
    const raw = fs.readFileSync(RUNTIME, "utf8");
    const parsed = JSON.parse(raw);
    return typeof parsed.port === "number" ? parsed.port : null;
  } catch {
    return null;
  }
}

async function main() {
  if (!fs.existsSync(BIN)) {
    throw new Error(`Packaged app binary missing: ${BIN}`);
  }
  if (!fs.existsSync(ENTRY)) {
    throw new Error(`Packaged backend entry missing: ${ENTRY}`);
  }

  fs.rmSync(WORKSPACE, { recursive: true, force: true });
  fs.mkdirSync(WORKSPACE, { recursive: true });

  const child = spawn(BIN, [ENTRY], {
    cwd: BACKEND,
    env: {
      ...process.env,
      ELECTRON_RUN_AS_NODE: "1",
      PORT: "0",
      FRONTEND_URL: "http://localhost:3000",
      JWT_SECRET: "packaged-backend-smoke-jwt",
      DOWNLOAD_SIGNING_SECRET: "packaged-backend-smoke-download",
      WORKSPACE_PATH: WORKSPACE,
      LOCAL_USER_ID: "local-user",
      LOCAL_USER_EMAIL: "user@local",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });

  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => {
    stdout += chunk.toString();
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk.toString();
  });

  try {
    const deadline = Date.now() + 15_000;
    while (Date.now() < deadline) {
      if (child.exitCode !== null) {
        throw new Error(`backend exited early with code ${child.exitCode}`);
      }
      const port = readRuntimePort();
      if (port && (await health(port).catch(() => false))) {
        console.log(`PACKAGED BACKEND SMOKE: PASS port=${port}`);
        return;
      }
      await sleep(250);
    }
    throw new Error("backend did not become healthy within 15s");
  } finally {
    if (child.exitCode === null) child.kill();
    if (stdout.trim()) console.log("---STDOUT---\n" + stdout.trim());
    if (stderr.trim()) console.log("---STDERR---\n" + stderr.trim());
  }
}

main().catch((err) => {
  console.error("PACKAGED BACKEND SMOKE: FAIL");
  console.error(err);
  process.exit(1);
});
