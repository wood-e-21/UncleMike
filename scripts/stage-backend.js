#!/usr/bin/env node
const fs = require("fs");
const path = require("path");

const root = path.resolve(__dirname, "..");
const stage = path.join(root, "backend", ".dist-bundle");
const binName = process.platform === "win32" ? "mike-backend.exe" : "mike-backend";
const built = path.join(root, "target", "release", binName);

fs.rmSync(stage, { recursive: true, force: true });
fs.mkdirSync(stage, { recursive: true });

if (!fs.existsSync(built)) {
  console.error(`[stage-backend] missing ${built}; run npm run build:backend first`);
  process.exit(1);
}

fs.copyFileSync(built, path.join(stage, binName));
console.log(`[stage-backend] staged ${binName}`);
