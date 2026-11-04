"use client";

import { useEffect, useState, useCallback } from "react";
import { useRouter, useParams } from "next/navigation";
import { Card } from "@/components/card";
import { PrlEditor } from "@/components/prl-editor/PrlEditor";
import { PrlDecoded } from "@/lib/proto/hlr/v1/service";
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
  const [loadError, setLoadError] = useState<unknown>(null);
  const [actionError, setActionError] = useState<unknown>(null);
  const [name, setName] = useState("");
  const [notes, setNotes] = useState("");
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState<unknown>(null);

  const load = useCallback(async () => {
    try {
      const data = await apiFetch<{ prl?: PrlFull }>(`/api/prls/${params.id}`, undefined, "load the PRL");
      setPrl(data.prl ?? null);
      setName(data.prl?.summary?.name ?? "");
      setNotes(data.prl?.summary?.notes ?? "");
      setLoadError(null);
    } catch (err) {
      setLoadError(err);
    }
  }, [params.id]);

  useEffect(() => {
    load();
  }, [load]);

  const saveBody = async (built: PrlDecoded) => {
    setBusy(true);
    setSaveError(null);
    try {
      await apiFetch(
        `/api/prls/${params.id}`,
        {
          method: "PATCH",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ built, name: name.trim(), notes }),
        },
        "save the PRL",
      );
      await load();
    } catch (err) {
      setSaveError(err);
    } finally {
      setBusy(false);
    }
  };

  const setDefault = async () => {
    setBusy(true);
    setActionError(null);
    try {
      await apiFetch(`/api/prls/${params.id}/set-default`, { method: "POST" }, "make this the default PRL");
      setPrl((current) =>
        current?.summary
          ? {
              ...current,
              summary: { ...current.summary, isDefault: true },
            }
          : current,
      );
    } catch (err) {
      setActionError(err);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!window.confirm(`Delete PRL "${prl?.summary?.name}"?`)) return;
    setBusy(true);
    setActionError(null);
    try {
      await apiFetch(`/api/prls/${params.id}`, { method: "DELETE" }, "delete the PRL");
      router.push("/prls");
    } catch (err) {
      setActionError(err);
    } finally {
      setBusy(false);
    }
  };

  if (loadError) return <ErrorNotice error={loadError} boxed onRetry={load} />;
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
      error={saveError ?? actionError}
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
