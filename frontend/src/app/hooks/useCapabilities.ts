"use client";

import { useEffect, useState } from "react";

export interface Capabilities {
    libreoffice: {
        available: boolean;
        version: string | null;
        install_url: string | null;
    };
}

let cached: Capabilities | null = null;
let inflight: Promise<Capabilities | null> | null = null;

async function fetchCapabilities(): Promise<Capabilities | null> {
    if (cached) return cached;
    if (inflight) return inflight;
    inflight = (async () => {
        cached = {
            libreoffice: {
                available: false,
                version: null,
                install_url: null,
            },
        };
        return cached;
    })();
    return inflight;
}

export function useCapabilities(): {
    capabilities: Capabilities | null;
    loading: boolean;
} {
    const [capabilities, setCapabilities] = useState<Capabilities | null>(
        cached,
    );
    const [loading, setLoading] = useState(!cached);

    useEffect(() => {
        if (cached) return;
        let mounted = true;
        fetchCapabilities()
            .then((c) => {
                if (mounted) setCapabilities(c);
            })
            .finally(() => {
                if (mounted) setLoading(false);
            });
        return () => {
            mounted = false;
        };
    }, []);

    return { capabilities, loading };
}
