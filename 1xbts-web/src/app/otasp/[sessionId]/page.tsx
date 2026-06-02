"use client";

import { use, useEffect, useState } from "react";
import Link from "next/link";
import { OtaspSessionDetail } from "@/components/otasp-session-detail";

export default function OtaspSessionDetailPage({
  params,
}: {
  params: Promise<{ sessionId: string }>;
}) {
  const { sessionId } = use(params);
  const [session, setSession] = useState<unknown | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const res = await fetch(`/api/otasp-events/${encodeURIComponent(sessionId)}`);
        if (!res.ok) {
          const body = await res.json().catch(() => ({}));
          throw new Error(body.error || `HTTP ${res.status}`);
        }
        const data = await res.json();
        if (!alive) return;
        if (data.error) throw new Error(data.error);
        setSession(data.session);
        setError(null);
      } catch (err) {
        if (!alive) return;
        setError(err instanceof Error ? err.message : "unknown");
      } finally {
        if (alive) setLoading(false);
      }
    };
    load();
    return () => {
      alive = false;
    };
  }, [sessionId]);

  return (
    <div className="max-w-5xl mx-auto space-y-4">
      <Link href="/mobiles" className="text-sm text-muted hover:text-secondary">
        &larr; Mobiles
      </Link>
      <h1 className="text-lg font-bold font-mono">OTASP Session</h1>
      {loading ? (
        <p className="text-dimmed text-sm">Loading...</p>
      ) : error ? (
        <div className="rounded-lg border border-accent-red/20 bg-accent-red-bg p-4 text-accent-red text-sm">
          {error}
        </div>
      ) : session ? (
        <OtaspSessionDetail session={session as Parameters<typeof OtaspSessionDetail>[0]["session"]} />
      ) : (
        <p className="text-dimmed text-sm">No session data.</p>
      )}
    </div>
  );
}
