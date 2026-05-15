"use client";

import { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { getApiBase } from "@/app/lib/mikeApi";
import { supabase } from "@/lib/supabase";

type Transport = "http" | "sse" | "stdio";

interface McpServer {
    name: string;
    transport: Transport;
    url?: string | null;
    command?: string | null;
    args?: string[];
    enabled: boolean;
}

async function api<T>(path: string, init?: RequestInit): Promise<T> {
    const {
        data: { session },
    } = await supabase.auth.getSession();
    const res = await fetch(`${await getApiBase()}${path}`, {
        ...init,
        headers: {
            "Content-Type": "application/json",
            ...(session?.access_token
                ? { Authorization: `Bearer ${session.access_token}` }
                : {}),
            ...(init?.headers as Record<string, string> | undefined),
        },
    });
    const text = await res.text();
    const data = text ? JSON.parse(text) : {};
    if (!res.ok) throw new Error(data.detail || `HTTP ${res.status}`);
    return data as T;
}

export default function McpServersPage() {
    const [servers, setServers] = useState<McpServer[]>([]);
    const [name, setName] = useState("");
    const [transport, setTransport] = useState<Transport>("http");
    const [endpoint, setEndpoint] = useState("");
    const [error, setError] = useState<string | null>(null);
    const [loading, setLoading] = useState(true);

    async function reload() {
        setLoading(true);
        try {
            const data = await api<{ servers: McpServer[] }>("/user/mcp-servers");
            setServers(data.servers ?? []);
            setError(null);
        } catch (err) {
            setError((err as Error).message);
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        void reload();
    }, []);

    async function addServer() {
        if (!name.trim()) return;
        const body =
            transport === "stdio"
                ? { name: name.trim(), transport, command: endpoint.trim(), enabled: true }
                : { name: name.trim(), transport, url: endpoint.trim(), enabled: true };
        await api("/user/mcp-servers", {
            method: "POST",
            body: JSON.stringify({ args: [], env: {}, headers: {}, ...body }),
        });
        setName("");
        setEndpoint("");
        await reload();
    }

    async function deleteServer(serverName: string) {
        await api(`/user/mcp-servers/${encodeURIComponent(serverName)}`, {
            method: "DELETE",
        });
        await reload();
    }

    return (
        <div className="max-w-3xl space-y-6">
            <div>
                <h2 className="text-2xl font-medium font-serif">MCP Servers</h2>
                <p className="mt-1 text-sm text-gray-500">
                    Configure local tool servers for assistant calls.
                </p>
            </div>

            {error && (
                <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
                    {error}
                </div>
            )}

            <div className="grid gap-2 md:grid-cols-[1fr_120px_1.5fr_auto]">
                <input
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="Name"
                    className="rounded-md border border-gray-200 px-3 py-2 text-sm"
                />
                <select
                    value={transport}
                    onChange={(e) => setTransport(e.target.value as Transport)}
                    className="rounded-md border border-gray-200 px-3 py-2 text-sm"
                >
                    <option value="http">HTTP</option>
                    <option value="sse">SSE</option>
                    <option value="stdio">stdio</option>
                </select>
                <input
                    value={endpoint}
                    onChange={(e) => setEndpoint(e.target.value)}
                    placeholder={transport === "stdio" ? "Command" : "URL"}
                    className="rounded-md border border-gray-200 px-3 py-2 text-sm"
                />
                <button
                    onClick={() => void addServer()}
                    className="inline-flex items-center justify-center gap-1 rounded-md bg-gray-900 px-3 py-2 text-sm text-white"
                >
                    <Plus className="h-4 w-4" />
                    Add
                </button>
            </div>

            {loading ? (
                <p className="text-sm text-gray-400">Loading...</p>
            ) : servers.length === 0 ? (
                <p className="rounded-md bg-gray-50 px-4 py-6 text-center text-sm text-gray-500">
                    No MCP servers configured.
                </p>
            ) : (
                <div className="divide-y divide-gray-100 rounded-md border border-gray-200">
                    {servers.map((server) => (
                        <div
                            key={server.name}
                            className="flex items-center justify-between gap-3 px-4 py-3"
                        >
                            <div className="min-w-0">
                                <div className="text-sm font-medium text-gray-900">
                                    {server.name}
                                </div>
                                <div className="truncate text-xs text-gray-500">
                                    {server.transport} ·{" "}
                                    {server.url || server.command || "not configured"}
                                </div>
                            </div>
                            <button
                                onClick={() => void deleteServer(server.name)}
                                className="rounded-md p-2 text-gray-400 hover:bg-red-50 hover:text-red-600"
                                aria-label={`Delete ${server.name}`}
                            >
                                <Trash2 className="h-4 w-4" />
                            </button>
                        </div>
                    ))}
                </div>
            )}
        </div>
    );
}
