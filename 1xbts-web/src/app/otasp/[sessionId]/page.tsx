"use client";

import { use, useEffect, useState } from "react";
import Link from "next/link";
import { OtaspSessionDetail } from "@/components/otasp-session-detail";
import { apiFetch } from "@/lib/api-client";
import { ErrorNotice } from "@/components/error-notice";

const SESSION_REFRESH_MS = 2000;

export default function OtaspSessionDetailPage({
  params,
}: {
  params: Promise<{ sessionId: string }>;
}) {
  const { sessionId } = use(params);
  const [session, setSession] = useState<unknown | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    let alive = true;
    let loadingSession = false;
    const load = async () => {
      if (loadingSession || !alive) return;
      loadingSession = true;
      try {
        const data = await apiFetch<{ session?: { summary?: { endedAt?: unknown } } }>(
          `/api/otasp-events/${encodeURIComponent(sessionId)}`,
          undefined,
          "load the OTASP session",
        );
        if (!alive) return;
        setSession(data.session);
        if (data.session?.summary?.endedAt && interval) clearInterval(interval);
        setError(null);
      } catch (err) {
        if (!alive) return;
        setError(err);
      } finally {
        loadingSession = false;
        if (alive) setLoading(false);
      }
    };
    const interval = setInterval(load, SESSION_REFRESH_MS);
    void load();
    return () => {
      alive = false;
      if (interval) clearInterval(interval);
    };
  }, [sessionId]);

  return (
    <div className="page-container space-y-4">
      <Link href="/mobiles" className="text-sm text-muted hover:text-secondary">
        &larr; Mobiles
      </Link>
      <h1 className="text-lg font-bold font-mono">OTASP Session</h1>
      {loading ? (
        <p className="text-dimmed text-sm">Loading...</p>
      ) : error ? (
        <ErrorNotice error={error} boxed />
      ) : session ? (
        <OtaspSessionDetail session={session as Parameters<typeof OtaspSessionDetail>[0]["session"]} />
      ) : (
        <p className="text-dimmed text-sm">No session data.</p>
      )}
    </div>
  );
}
