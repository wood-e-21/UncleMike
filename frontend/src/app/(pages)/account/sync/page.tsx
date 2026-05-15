"use client";

import { useEffect, useState } from "react";
import { Folder, RefreshCw, Trash2 } from "lucide-react";
import { getApiBase, listProjects } from "@/app/lib/mikeApi";
import type { MikeProject } from "@/app/components/shared/types";
import { supabase } from "@/lib/supabase";

interface SyncFolder {
    id: string;
    path: string;
    label: string | null;
    recursive: boolean;
    enabled: boolean;
    last_scan_at: string | null;
    project_id: string | null;
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

export default function SyncPage() {
    const [folders, setFolders] = useState<SyncFolder[]>([]);
    const [matters, setMatters] = useState<MikeProject[]>([]);
    const [path, setPath] = useState("");
    const [label, setLabel] = useState("");
    const [matterId, setMatterId] = useState("");
    const [error, setError] = useState<string | null>(null);
    const [loading, setLoading] = useState(true);

    async function reload() {
        setLoading(true);
        try {
            const [syncFolders, projectList] = await Promise.all([
                api<SyncFolder[]>("/sync/folders"),
                listProjects(),
            ]);
            setFolders(syncFolders);
            setMatters(projectList);
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

    async function addFolder() {
        if (!path.trim()) return;
        await api("/sync/folders", {
            method: "POST",
            body: JSON.stringify({
                path: path.trim(),
                label: label.trim() || null,
                recursive: true,
                enabled: true,
                project_id: matterId || null,
            }),
        });
        setPath("");
        setLabel("");
        await reload();
    }

    async function scanFolder(id: string) {
        await api(`/sync/folders/${id}/scan`, { method: "POST" });
        await reload();
    }

    async function deleteFolder(id: string) {
        await api(`/sync/folders/${id}`, { method: "DELETE" });
        await reload();
    }

    return (
        <div className="max-w-4xl space-y-6">
            <div>
                <h2 className="text-2xl font-medium font-serif">Folder Sync</h2>
                <p className="mt-1 text-sm text-gray-500">
                    Index local folders into the global library or a selected matter.
                </p>
            </div>

            {error && (
                <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
                    {error}
                </div>
            )}

            <div className="grid gap-2 md:grid-cols-[1.5fr_1fr_1fr_auto]">
                <input
                    value={path}
                    onChange={(e) => setPath(e.target.value)}
                    placeholder="/Users/name/Documents/Folder"
                    className="rounded-md border border-gray-200 px-3 py-2 text-sm"
                />
                <input
                    value={label}
                    onChange={(e) => setLabel(e.target.value)}
                    placeholder="Label"
                    className="rounded-md border border-gray-200 px-3 py-2 text-sm"
                />
                <select
                    value={matterId}
                    onChange={(e) => setMatterId(e.target.value)}
                    className="rounded-md border border-gray-200 px-3 py-2 text-sm"
                >
                    <option value="">Global library</option>
                    {matters.map((matter) => (
                        <option key={matter.id} value={matter.id}>
                            {matter.name}
                        </option>
                    ))}
                </select>
                <button
                    onClick={() => void addFolder()}
                    className="rounded-md bg-gray-900 px-3 py-2 text-sm text-white"
                >
                    Add
                </button>
            </div>

            {loading ? (
                <p className="text-sm text-gray-400">Loading...</p>
            ) : folders.length === 0 ? (
                <p className="rounded-md bg-gray-50 px-4 py-6 text-center text-sm text-gray-500">
                    No sync folders configured.
                </p>
            ) : (
                <div className="divide-y divide-gray-100 rounded-md border border-gray-200">
                    {folders.map((folder) => {
                        const matter = matters.find((m) => m.id === folder.project_id);
                        return (
                            <div
                                key={folder.id}
                                className="flex items-center justify-between gap-3 px-4 py-3"
                            >
                                <div className="flex min-w-0 items-center gap-3">
                                    <Folder className="h-4 w-4 shrink-0 text-gray-400" />
                                    <div className="min-w-0">
                                        <div className="truncate text-sm font-medium text-gray-900">
                                            {folder.label || folder.path}
                                        </div>
                                        <div className="truncate text-xs text-gray-500">
                                            {folder.path} · {matter?.name || "Global library"}
                                        </div>
                                    </div>
                                </div>
                                <div className="flex items-center gap-1">
                                    <button
                                        onClick={() => void scanFolder(folder.id)}
                                        className="rounded-md p-2 text-gray-400 hover:bg-gray-100 hover:text-gray-900"
                                        aria-label={`Scan ${folder.path}`}
                                    >
                                        <RefreshCw className="h-4 w-4" />
                                    </button>
                                    <button
                                        onClick={() => void deleteFolder(folder.id)}
                                        className="rounded-md p-2 text-gray-400 hover:bg-red-50 hover:text-red-600"
                                        aria-label={`Delete ${folder.path}`}
                                    >
                                        <Trash2 className="h-4 w-4" />
                                    </button>
                                </div>
                            </div>
                        );
                    })}
                </div>
            )}
        </div>
    );
}
