import type { NextConfig } from "next";

const nextConfig: NextConfig = {
    // Standalone output bundles a minimal Node server in
    // .next/standalone/server.js. The Electron main process spawns it in prod.
    output: "standalone",
    reactCompiler: true,
    skipTrailingSlashRedirect: true,
};

export default nextConfig;
