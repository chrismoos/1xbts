"use client";

import { useEffect, useState } from "react";
import Link from "next/link";

const SESSION_WAIT_MS = 90000;
const SESSION_POLL_MS = 2000;

export function StartOtasp({ subscriberId, imsi }: { subscriberId: string; imsi?: string }) {
  const [pending, setPending] = useState(false);
  const [previousSessions, setPreviousSessions] = useState<string[] | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const sessionsUrl = `/api/otasp-sessions?subscriberId=${encodeURIComponent(subscriberId)}&limit=10`;

  useEffect(() => {
    if (!previousSessions) return;
    let alive = true;
    const abort = new AbortController();
    const deadline = Date.now() + SESSION_WAIT_MS;
    const poll = async () => {
      try {
        const response = await fetch(sessionsUrl, { signal: abort.signal });
        if (response.ok) {
          const result = await response.json();
          const session = result.sessions?.find((row: { sessionId: string; featureCode?: string }) =>
            !row.featureCode && !previousSessions.includes(row.sessionId));
          if (alive && session) {
            setSessionId(session.sessionId);
            setMessage("Provisioning session started.");
            setPreviousSessions(null);
            setPending(false);
            return;
          }
        }
      } catch {
        if (!alive) return;
      }
      if (alive && Date.now() >= deadline) {
        setError("No provisioning session has appeared yet. Check handset reachability and support, or retry after it returns to service.");
        setMessage(null);
        setPreviousSessions(null);
        setPending(false);
      }
    };
    const interval = setInterval(poll, SESSION_POLL_MS);
    void poll();
    return () => {
      alive = false;
      abort.abort();
      clearInterval(interval);
    };
  }, [previousSessions, sessionsUrl]);

  async function start() {
    setPending(true);
    setError(null);
    setMessage(null);
    setSessionId(null);
    try {
      const existing = await fetch(sessionsUrl);
      if (!existing.ok) throw new Error("Session history is unavailable. Try again when the HLR is connected.");
      const history = await existing.json();
      const response = await fetch(`/api/subscribers/${encodeURIComponent(subscriberId)}/otasp`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ imsi }) });
      const result = await response.json();
      if (!response.ok || !result.accepted) throw new Error(result.error || "Provisioning could not be started.");
      setMessage("Paging handset. A recently rejected registration can delay its response.");
      setPreviousSessions((history.sessions || []).map((row: { sessionId: string }) => row.sessionId));
    } catch (error) {
      setError(error instanceof Error ? error.message : "Provisioning could not be started.");
      setPending(false);
    }
  }

  return (
    <div className="space-y-2">
      <button type="button" onClick={start} disabled={pending}
        className="rounded border border-accent-green/20 bg-accent-green-bg px-3 py-1.5 text-xs text-accent-green transition-colors hover:bg-accent-green/15 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent-green disabled:opacity-50">
        {pending ? "Starting provisioning…" : "Start provisioning"}
      </button>
      <p className="text-xs text-muted">Pages the handset and applies the configured OTASP settings and subscriber overrides.</p>
      {message && <p role="status" className="text-xs text-secondary">{message}</p>}
      {sessionId && <Link href={`/otasp/${encodeURIComponent(sessionId)}`} className="text-xs text-accent-green underline underline-offset-2">View session progress</Link>}
      {error && <p role="alert" className="text-xs text-accent-red">{error}</p>}
    </div>
  );
}
