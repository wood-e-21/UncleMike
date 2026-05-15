// Copies non-TypeScript Electron assets (preload, lock screen) into dist-electron
// after tsc compilation. Keeps a single load path for both dev and prod.

const fs = require("fs");
const path = require("path");

const root = path.resolve(__dirname, "..");
const src = path.join(root, "electron");
const out = path.join(root, "dist-electron");

function copyDir(srcDir, destDir) {
  fs.mkdirSync(destDir, { recursive: true });
  for (const entry of fs.readdirSync(srcDir, { withFileTypes: true })) {
    const s = path.join(srcDir, entry.name);
    const d = path.join(destDir, entry.name);
    if (entry.isDirectory()) {
      copyDir(s, d);
    } else if (entry.isFile()) {
      fs.copyFileSync(s, d);
    }
  }
}

fs.mkdirSync(out, { recursive: true });
fs.copyFileSync(path.join(src, "preload.js"), path.join(out, "preload.js"));
copyDir(path.join(src, "lock"), path.join(out, "lock"));

console.log("Copied preload.js and lock/ to dist-electron/");
