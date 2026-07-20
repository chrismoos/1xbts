"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import {
  SessionState,
  type Session,
  type GetUatiAllocationResponse,
} from "@/lib/proto/an/v1/service";

/// Dashboard rollup of the EV-DO (HRPD) access network: session count by state
/// and UATI pool utilization. Quietly shows "not running" when the AN service
/// is unreachable (e.g. EV-DO disabled).
export function HrpdSummaryCard() {
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [allocation, setAllocation] = useState<GetUatiAllocationResponse | null>(
    null,
  );
  const [unavailable, setUnavailable] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      fetch("/api/an-sessions")
        .then((r) => r.json())
        .then((d: { sessions?: Session[]; error?: string }) => {
          if (cancelled) return;
          if (d.error) {
            setUnavailable(true);
          } else {
            setUnavailable(false);
            setSessions(d.sessions ?? []);
          }
        })
        .catch(() => {
          if (!cancelled) setUnavailable(true);
        });
      fetch("/api/an-uati-allocation")
        .then((r) => r.json())
        .then((d: GetUatiAllocationResponse & { error?: string }) => {
          if (cancelled || d.error) return;
          setAllocation(d);
        })
        .catch(() => {});
    };
    tick();
    const id = setInterval(tick, 3000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  const openCount =
    sessions?.filter((s) => s.state === SessionState.SESSION_STATE_OPEN)
      .length ?? 0;
  const usedPct =
    allocation && allocation.capacity > 0
      ? Math.min(100, (allocation.inUse / allocation.capacity) * 100)
      : 0;

  return (
    <Card title="EV-DO (HRPD)">
      {unavailable ? (
        <p className="text-dimmed text-sm">Not running</p>
      ) : (
        <div className="space-y-3 text-xs">
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-muted">
            <span>
              <span className="text-primary font-mono">
                {sessions?.length ?? 0}
              </span>{" "}
              sessions
            </span>
            <span>
              <span className="text-accent-green font-mono">{openCount}</span>{" "}
              open
            </span>
          </div>
          {allocation && (
            <div className="space-y-1">
              <div className="flex justify-between text-muted">
                <span>UATI pool (color {allocation.colorCode})</span>
                <span className="font-mono">
                  {allocation.inUse}/{allocation.capacity}
                </span>
              </div>
              <div className="h-1.5 w-full rounded bg-surface-raised overflow-hidden">
                <div
                  className="h-full bg-accent-indigo transition-all"
                  style={{ width: `${usedPct}%` }}
                />
              </div>
            </div>
          )}
        </div>
      )}
      <div className="mt-2 pt-2 border-t border-border">
        <Link
          href="/hrpd"
          className="text-xs text-accent-green hover:text-accent-green"
        >
          HRPD sessions →
        </Link>
      </div>
    </Card>
  );
}
