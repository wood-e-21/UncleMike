"use client";

import React, {
    createContext,
    useContext,
    useEffect,
    useState,
    ReactNode,
    useCallback,
} from "react";
import { supabase } from "@/lib/supabase";
import { useAuth } from "@/contexts/AuthContext";
import { getApiBase } from "@/app/lib/mikeApi";

interface UserProfile {
    displayName: string | null;
    organisation: string | null;
    messageCreditsUsed: number;
    creditsResetDate: string;
    creditsRemaining: number;
    tier: string;
    tabularModel: string;
    claudeApiKey: string | null;
    geminiApiKey: string | null;
}

interface UserProfileContextType {
    profile: UserProfile | null;
    loading: boolean;
    updateDisplayName: (name: string) => Promise<boolean>;
    updateOrganisation: (organisation: string) => Promise<boolean>;
    updateModelPreference: (
        field: "tabularModel",
        value: string,
    ) => Promise<boolean>;
    updateApiKey: (
        provider: "claude" | "gemini",
        value: string | null,
    ) => Promise<boolean>;
    reloadProfile: () => Promise<void>;
}

const UserProfileContext = createContext<UserProfileContextType | undefined>(
    undefined,
);

// Local desktop build: credit metering is meaningless (no SaaS billing).
// Keep the field set high so all UI paths gating on it pass.
const UNMETERED = 999_999;

interface ServerProfile {
    user_id: string;
    display_name: string | null;
    organisation: string | null;
    tier: string | null;
    message_credits_used: number;
    credits_reset_date: string;
    tabular_model: string | null;
    claude_api_key: string | null;
    gemini_api_key: string | null;
}

function toClientProfile(p: ServerProfile | null): UserProfile {
    return {
        displayName: p?.display_name ?? null,
        organisation: p?.organisation ?? null,
        messageCreditsUsed: p?.message_credits_used ?? 0,
        creditsResetDate:
            p?.credits_reset_date ?? new Date().toISOString(),
        creditsRemaining: UNMETERED,
        tier: p?.tier ?? "Free",
        tabularModel: p?.tabular_model ?? "gemini-3-flash-preview",
        claudeApiKey: p?.claude_api_key ?? null,
        geminiApiKey: p?.gemini_api_key ?? null,
    };
}

async function authHeaders(): Promise<Record<string, string>> {
    const {
        data: { session },
    } = await supabase.auth.getSession();
    return session?.access_token
        ? { Authorization: `Bearer ${session.access_token}` }
        : {};
}

async function fetchProfile(): Promise<ServerProfile | null> {
    const headers = await authHeaders();
    const resp = await fetch(`${await getApiBase()}/user/profile`, { headers });
    if (!resp.ok) return null;
    return (await resp.json()) as ServerProfile;
}

async function patchProfile(
    update: Partial<ServerProfile>,
): Promise<ServerProfile | null> {
    const headers = await authHeaders();
    const resp = await fetch(`${await getApiBase()}/user/profile`, {
        method: "PATCH",
        headers: { ...headers, "Content-Type": "application/json" },
        body: JSON.stringify(update),
    });
    if (!resp.ok) return null;
    return (await resp.json()) as ServerProfile;
}

export function UserProfileProvider({ children }: { children: ReactNode }) {
    const { user, isAuthenticated } = useAuth();
    const [profile, setProfile] = useState<UserProfile | null>(null);
    const [loading, setLoading] = useState(true);

    const loadProfile = useCallback(async () => {
        const server = await fetchProfile();
        setProfile(toClientProfile(server));
    }, []);

    useEffect(() => {
        if (isAuthenticated && user) {
            setLoading(true);
            loadProfile().finally(() => setLoading(false));
        } else {
            setProfile(null);
            setLoading(false);
        }
    }, [isAuthenticated, user, loadProfile]);

    const updateDisplayName = useCallback(
        async (displayName: string): Promise<boolean> => {
            const updated = await patchProfile({ display_name: displayName });
            if (!updated) return false;
            setProfile(toClientProfile(updated));
            return true;
        },
        [],
    );

    const updateOrganisation = useCallback(
        async (organisation: string): Promise<boolean> => {
            const updated = await patchProfile({ organisation });
            if (!updated) return false;
            setProfile(toClientProfile(updated));
            return true;
        },
        [],
    );

    const updateModelPreference = useCallback(
        async (field: "tabularModel", value: string): Promise<boolean> => {
            if (field !== "tabularModel") return false;
            const updated = await patchProfile({ tabular_model: value });
            if (!updated) return false;
            setProfile(toClientProfile(updated));
            return true;
        },
        [],
    );

    const updateApiKey = useCallback(
        async (
            provider: "claude" | "gemini",
            value: string | null,
        ): Promise<boolean> => {
            const dbField =
                provider === "claude" ? "claude_api_key" : "gemini_api_key";
            const normalized = value?.trim() ? value.trim() : null;
            const updated = await patchProfile({ [dbField]: normalized });
            if (!updated) return false;
            setProfile(toClientProfile(updated));
            return true;
        },
        [],
    );

    const reloadProfile = useCallback(async () => {
        await loadProfile();
    }, [loadProfile]);

    return (
        <UserProfileContext.Provider
            value={{
                profile,
                loading,
                updateDisplayName,
                updateOrganisation,
                updateModelPreference,
                updateApiKey,
                reloadProfile,
            }}
        >
            {children}
        </UserProfileContext.Provider>
    );
}

export function useUserProfile() {
    const context = useContext(UserProfileContext);
    if (context === undefined) {
        throw new Error(
            "useUserProfile must be used within a UserProfileProvider",
        );
    }
    return context;
}
