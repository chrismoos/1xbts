"use client";

import { useEffect, useState, useCallback } from "react";
import { useRouter, useParams } from "next/navigation";
import { Card } from "@/components/card";
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

  const saveBody = async (built: PrlDecoded) => {
    setBusy(true);
    setSaveError(undefined);
    try {
      const res = await fetch(`/api/prls/${params.id}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ built, name: name.trim(), notes }),
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
      else {
        setPrl((current) =>
          current?.summary
            ? {
                ...current,
                summary: { ...current.summary, isDefault: true },
              }
            : current,
        );
      }
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
  if (!prl.decoded || !s) {
    return (
      <Card title="PRL unavailable" className="border-accent-red/40">
        <p className="text-accent-red text-sm">
          This PRL does not have an editable decoded body.
        </p>
      </Card>
    );
  }

  return (
    <PrlEditor
      decoded={prl.decoded}
      onSave={saveBody}
      saving={busy}
      error={saveError}
      metadata={{
        savedName: s.name,
        name,
        savedNotes: s.notes,
        notes,
        isDefault: s.isDefault,
        rawBytesSize: s.rawBytesSize,
        busy,
        onNameChange: setName,
        onNotesChange: setNotes,
        onSetDefault: setDefault,
        onDelete: remove,
      }}
      metadataDirty={name !== s.name || notes !== s.notes}
    />
  );
}
