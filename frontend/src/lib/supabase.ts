/**
 * Compatibility shim for the local desktop build.
 *
 * The original web app called `supabase.auth.getSession()` directly from many
 * components and hooks. In the local desktop build, the JWT comes from the
 * Electron main process (the lock screen issues it after the user enters their
 * password). This shim exposes the same `supabase.auth.*` surface the rest of
 * the app already calls, but backs it with `window.mike.*` IPC.
 *
 * `supabase.from(...)` is intentionally not implemented — direct DB access from
 * the browser is replaced by routed backend calls in PHASE-04. Any caller still
 * reaching for it will throw, which is a useful signal during the migration.
 */

interface MikeBridge {
    getToken: () => Promise<string | null>;
    getUser: () => Promise<{ id: string; email: string } | null>;
    getApiBase?: () => Promise<string>;
    signOut?: () => Promise<{ ok: boolean }>;
}

declare global {
    interface Window {
        mike?: MikeBridge & Record<string, unknown>;
    }
}

interface Session {
    access_token: string;
    user: { id: string; email: string };
}

interface AuthChangeListener {
    (event: string, session: Session | null): void | Promise<void>;
}

// The JWT and user identity are stable for the lifetime of an unlock —
// Electron mints them once at unlock and they never rotate until signOut.
// Cache the first read so we don't pay an IPC round-trip on every API
// call (the chat hot path was hitting IPC on each `getAuthHeader()`).
type CachedBridge = {
    token: string;
    user: { id: string; email: string };
} | null;

let cachedBridge: CachedBridge = null;
let inflightBridge: Promise<CachedBridge> | null = null;

async function readBridge(): Promise<CachedBridge> {
    if (cachedBridge) return cachedBridge;
    if (inflightBridge) return inflightBridge;
    if (typeof window === "undefined") return null;
    const bridge = window.mike;
    if (!bridge?.getToken || !bridge?.getUser) return null;
    inflightBridge = (async () => {
        const [token, user] = await Promise.all([
            bridge.getToken(),
            bridge.getUser(),
        ]);
        if (!token || !user) return null;
        cachedBridge = { token, user };
        return cachedBridge;
    })();
    try {
        return await inflightBridge;
    } finally {
        inflightBridge = null;
    }
}

function clearBridgeCache(): void {
    cachedBridge = null;
    inflightBridge = null;
}

export const supabase = {
    auth: {
        async getSession(): Promise<{ data: { session: Session | null }; error: null }> {
            const bridge = await readBridge();
            if (!bridge) return { data: { session: null }, error: null };
            return {
                data: {
                    session: {
                        access_token: bridge.token,
                        user: bridge.user,
                    },
                },
                error: null,
            };
        },
        async getUser(_token?: string): Promise<{ data: { user: { id: string; email: string } | null }; error: null }> {
            const bridge = await readBridge();
            return { data: { user: bridge?.user ?? null }, error: null };
        },
        async signOut(): Promise<{ error: null }> {
            // B6: ask the Electron main process to tear down the session, kill
            // the backend, and return to the lock screen. Without this, the
            // renderer keeps a valid JWT in memory after the user clicks
            // "sign out" — making the affordance a lie.
            clearBridgeCache();
            if (typeof window !== "undefined") {
                try {
                    await window.mike?.signOut?.();
                } catch {
                    // main process will reload the lock screen anyway
                }
            }
            return { error: null };
        },
        onAuthStateChange(_cb: AuthChangeListener): {
            data: { subscription: { unsubscribe: () => void } };
        } {
            // Auth state in the desktop app is set once at unlock and doesn't
            // change while the window is open. Returning a no-op subscription
            // keeps existing callers happy.
            return {
                data: { subscription: { unsubscribe: () => {} } },
            };
        },
    },
    from(_table: string): never {
        throw new Error(
            "Direct database access via supabase.from() is not supported in the local desktop build. Route the call through the backend API.",
        );
    },
};
