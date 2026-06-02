"use client";

import { useEffect, useState, useCallback } from "react";
import { useRouter, useParams } from "next/navigation";
import Link from "next/link";
import { Card, Stat } from "@/components/card";
import { PrlEditor } from "@/components/prl-editor/PrlEditor";
import { PrlDecoded } from "@/lib/proto/hlr/v1/service";

interface PrlSummary {
  prlId: string;
  name: string;
  prListId: number;
  ssprPRev: number;
  isDefault: boolean;
  rawBytesSize: number;
  notes: string;
}

interface PrlFull {
  summary?: PrlSummary;
  rawBytes?: string;
  decoded?: PrlDecoded;
}

export default function PrlDetailPage() {
  const params = useParams<{ id: string }>();
  const router = useRouter();
  const [prl, setPrl] = useState<PrlFull | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [notes, setNotes] = useState("");
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState<string | undefined>(undefined);

  const load = useCallback(async () => {
    const res = await fetch(`/api/prls/${params.id}`);
    const data = await res.json();
    if (data.error) {
      setError(data.error);
    } else {
      setPrl(data.prl);
      setName(data.prl?.summary?.name ?? "");
      setNotes(data.prl?.summary?.notes ?? "");
      setError(null);
    }
  }, [params.id]);

  useEffect(() => {
    load();
  }, [load]);

  const saveMetadata = async () => {
    setBusy(true);
    try {
      const res = await fetch(`/api/prls/${params.id}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, notes }),
      });
      const data = await res.json();
      if (data.error) setError(data.error);
      else await load();
    } finally {
      setBusy(false);
    }
  };

  const saveBody = async (built: PrlDecoded) => {
    setBusy(true);
    setSaveError(undefined);
    try {
      const res = await fetch(`/api/prls/${params.id}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ built }),
      });
      const data = await res.json();
      if (data.error) setSaveError(data.error);
      else await load();
    } finally {
      setBusy(false);
    }
  };

  const setDefault = async () => {
    setBusy(true);
    try {
      const res = await fetch(`/api/prls/${params.id}/set-default`, {
        method: "POST",
      });
      const data = await res.json();
      if (data.error) setError(data.error);
      else await load();
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!window.confirm(`Delete PRL "${prl?.summary?.name}"?`)) return;
    setBusy(true);
    try {
      const res = await fetch(`/api/prls/${params.id}`, { method: "DELETE" });
      const data = await res.json();
      if (data.error) setError(data.error);
      else router.push("/prls");
    } finally {
      setBusy(false);
    }
  };

  if (error)
    return (
      <Card title="Error" className="border-accent-red/40">
        <p className="text-accent-red text-sm">{error}</p>
      </Card>
    );
  if (!prl) return <p className="text-dimmed text-xs">Loading…</p>;

  const s = prl.summary;

  return (
    <div className="space-y-4">
      <div className="flex items-baseline gap-3">
        <Link href="/prls" className="text-muted text-xs hover:text-primary">
          ← All PRLs
        </Link>
        <h1 className="text-primary text-lg font-medium">{s?.name}</h1>
        {s?.isDefault && (
          <span className="text-accent-green text-xs">✓ default</span>
        )}
        <button
          onClick={remove}
          disabled={busy}
          className="ml-auto text-accent-red text-xs hover:underline disabled:opacity-50"
        >
          Delete
        </button>
      </div>

      <Card title="Metadata">
        <div className="grid grid-cols-2 gap-x-6 text-xs">
          <Stat
            label="PR_LIST_ID"
            value={`${s?.prListId} (0x${(s?.prListId ?? 0).toString(16).toUpperCase()})`}
            mono
          />
          <Stat
            label="Rev"
            value={s?.ssprPRev === 1 ? "Classic" : s?.ssprPRev === 3 ? "Extended" : `${s?.ssprPRev}`}
          />
          <Stat label="Size" value={`${s?.rawBytesSize ?? 0} octets`} mono />
          <Stat label="Default" value={s?.isDefault ? "yes" : "no"} />
        </div>
        <div className="mt-3 space-y-2 text-xs">
          <label className="block">
            <span className="text-muted">Name</span>
            <input
              className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </label>
          <label className="block">
            <span className="text-muted">Notes</span>
            <textarea
              className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1 font-mono"
              rows={2}
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
            />
          </label>
          <div className="flex gap-2">
            <button
              onClick={saveMetadata}
              disabled={busy}
              className="bg-accent-blue/20 border border-accent-blue/40 text-accent-blue px-3 py-1 rounded disabled:opacity-50"
            >
              Save name/notes
            </button>
            {!s?.isDefault && (
              <button
                onClick={setDefault}
                disabled={busy}
                className="text-accent-blue text-xs hover:underline"
              >
                Mark as default
              </button>
            )}
          </div>
        </div>
      </Card>

      {prl.decoded && (
        <PrlEditor
          decoded={prl.decoded}
          onSave={saveBody}
          saving={busy}
          error={saveError}
        />
      )}
    </div>
  );
}
