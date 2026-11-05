"use client";

import { useEffect, useState, useCallback } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import { formatTimeMs } from "@/lib/format";
import { apiFetch } from "@/lib/api-client";
import { ErrorNotice } from "@/components/error-notice";

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
  const [error, setError] = useState<unknown>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const data = await apiFetch<{ prls?: PrlSummary[] }>("/api/prls", undefined, "load PRLs");
      setPrls(data.prls ?? []);
      setError(null);
    } catch (e) {
      setError(e);
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
      await apiFetch(`/api/prls/${prlId}/set-default`, { method: "POST" }, "make this the default PRL");
      await load();
    } catch (e) {
      setError(e);
    } finally {
      setBusyId(null);
    }
  };

  const remove = async (prlId: string, name: string) => {
    if (!window.confirm(`Delete PRL "${name}"? This is permanent.`)) return;
    setBusyId(prlId);
    try {
      await apiFetch(`/api/prls/${prlId}`, { method: "DELETE" }, `delete PRL "${name}"`);
      await load();
    } catch (e) {
      setError(e);
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="page-container space-y-4">
      <div className="flex items-baseline gap-3">
        <h1 className="text-primary text-lg font-medium">PRLs</h1>
        <Link
          href="/prls/new"
          className="ml-auto text-accent-blue text-xs hover:underline"
        >
          + New PRL
        </Link>
      </div>

      <ErrorNotice error={error} boxed />

      <Card title="">
        {loading ? (
          <p className="text-dimmed text-xs">Loading…</p>
        ) : prls.length === 0 ? (
          <p className="text-dimmed text-xs">
            No PRLs yet. Upload a .prl file to get started.
          </p>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th className="font-normal">Name</th>
                <th className="font-normal">PR_LIST_ID</th>
                <th className="font-normal">Rev</th>
                <th className="font-normal">Size</th>
                <th className="font-normal">Default</th>
                <th className="font-normal">Updated</th>
                <th className="font-normal" />
              </tr>
            </thead>
            <tbody>
              {prls.map((p) => {
                const updated = timestampMs(p.updatedAt);
                return (
                  <tr key={p.prlId} className="border-border/30">
                    <td>
                      <Link
                        href={`/prls/${p.prlId}`}
                        className="text-primary hover:underline"
                      >
                        {p.name}
                      </Link>
                    </td>
                    <td className="font-mono">{p.prListId}</td>
                    <td>{revLabel(p.ssprPRev)}</td>
                    <td className="font-mono">{p.rawBytesSize} B</td>
                    <td>
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
                    <td>{updated ? formatTimeMs(updated) : "—"}</td>
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
