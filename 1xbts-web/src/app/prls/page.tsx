"use client";

import { useEffect, useState, useCallback } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import { formatTimeMs } from "@/lib/format";

interface PrlSummary {
  prlId: string;
  name: string;
  prListId: number;
  ssprPRev: number;
  isDefault: boolean;
  rawBytesSize: number;
  notes: string;
  createdAt?: { seconds: number; nanos: number };
  updatedAt?: { seconds: number; nanos: number };
}

function timestampMs(ts?: { seconds: number; nanos: number }): number | null {
  if (!ts) return null;
  return ts.seconds * 1000 + Math.round(ts.nanos / 1_000_000);
}

function revLabel(rev: number): string {
  if (rev === 1) return "Classic";
  if (rev === 3) return "Extended";
  return `SSPR_P_REV ${rev}`;
}

export default function PrlsPage() {
  const [prls, setPrls] = useState<PrlSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await fetch("/api/prls");
      const data = await res.json();
      if (data.error) {
        setError(data.error);
      } else {
        setPrls(data.prls ?? []);
        setError(null);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const setDefault = async (prlId: string) => {
    setBusyId(prlId);
    try {
      const res = await fetch(`/api/prls/${prlId}/set-default`, { method: "POST" });
      const data = await res.json();
      if (data.error) {
        setError(data.error);
      } else {
        await load();
      }
    } finally {
      setBusyId(null);
    }
  };

  const remove = async (prlId: string, name: string) => {
    if (!window.confirm(`Delete PRL "${name}"? This is permanent.`)) return;
    setBusyId(prlId);
    try {
      const res = await fetch(`/api/prls/${prlId}`, { method: "DELETE" });
      const data = await res.json();
      if (data.error) {
        setError(data.error);
      } else {
        await load();
      }
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex items-baseline gap-3">
        <h1 className="text-primary text-lg font-medium">PRLs</h1>
        <Link
          href="/prls/new"
          className="ml-auto text-accent-blue text-xs hover:underline"
        >
          + New PRL
        </Link>
      </div>

      {error && (
        <Card title="Error" className="border-accent-red/40">
          <p className="text-accent-red text-sm">{error}</p>
        </Card>
      )}

      <Card title="">
        {loading ? (
          <p className="text-dimmed text-xs">Loading…</p>
        ) : prls.length === 0 ? (
          <p className="text-dimmed text-xs">
            No PRLs yet. Upload a .prl file to get started.
          </p>
        ) : (
          <table className="w-full text-xs">
            <thead>
              <tr className="text-left text-muted">
                <th className="font-normal pr-3 py-1">Name</th>
                <th className="font-normal pr-3">PR_LIST_ID</th>
                <th className="font-normal pr-3">Rev</th>
                <th className="font-normal pr-3">Size</th>
                <th className="font-normal pr-3">Default</th>
                <th className="font-normal pr-3">Updated</th>
                <th className="font-normal" />
              </tr>
            </thead>
            <tbody>
              {prls.map((p) => {
                const updated = timestampMs(p.updatedAt);
                return (
                  <tr key={p.prlId} className="border-t border-border/30">
                    <td className="py-1 pr-3">
                      <Link
                        href={`/prls/${p.prlId}`}
                        className="text-primary hover:underline"
                      >
                        {p.name}
                      </Link>
                    </td>
                    <td className="font-mono pr-3">{p.prListId}</td>
                    <td className="pr-3">{revLabel(p.ssprPRev)}</td>
                    <td className="font-mono pr-3">{p.rawBytesSize} B</td>
                    <td className="pr-3">
                      {p.isDefault ? (
                        <span className="text-accent-green">✓ default</span>
                      ) : (
                        <button
                          onClick={() => setDefault(p.prlId)}
                          disabled={busyId === p.prlId}
                          className="text-accent-blue text-[11px] hover:underline disabled:opacity-50"
                        >
                          set default
                        </button>
                      )}
                    </td>
                    <td className="pr-3">{updated ? formatTimeMs(updated) : "—"}</td>
                    <td>
                      <button
                        onClick={() => remove(p.prlId, p.name)}
                        disabled={busyId === p.prlId}
                        className="text-accent-red text-[11px] hover:underline disabled:opacity-50"
                      >
                        delete
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </Card>
    </div>
  );
}
