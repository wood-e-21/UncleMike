"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useAuth } from "@/contexts/AuthContext";

export default function LoginPage() {
    const router = useRouter();
    const { isAuthenticated, authLoading } = useAuth();

    useEffect(() => {
        if (!authLoading && isAuthenticated) {
            router.replace("/assistant");
        }
    }, [authLoading, isAuthenticated, router]);

    return (
        <div className="flex h-dvh items-center justify-center bg-gray-50 px-4">
            <div className="max-w-md text-center text-gray-700">
                <h1 className="mb-3 text-2xl font-semibold text-gray-900">
                    Mike is locked
                </h1>
                <p className="text-sm leading-6">
                    Close this window and reopen the Mike desktop launcher to
                    enter your password.
                </p>
            </div>
        </div>
    );
}
