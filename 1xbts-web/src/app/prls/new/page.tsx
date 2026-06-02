"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { Card } from "@/components/card";
import { emptyClassicPrl, emptyExtendedPrl } from "@/lib/prl-empty";

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

type Mode = "upload" | "classic" | "extended";

export default function NewPrlPage() {
  const router = useRouter();
  const [mode, setMode] = useState<Mode>("upload");
  const [name, setName] = useState("");
  const [notes, setNotes] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [prListId, setPrListId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) {
      setError("Name is required.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      let body: Record<string, unknown>;
      if (mode === "upload") {
        if (!file) {
          setError("Upload a .prl file.");
          return;
        }
        const buffer = new Uint8Array(await file.arrayBuffer());
        body = {
          name: name.trim(),
          notes,
          rawBytesBase64: bytesToBase64(buffer),
        };
      } else {
        const id = Number(prListId);
        if (!Number.isFinite(id) || id < 0 || id > 0xffff) {
          setError("PR_LIST_ID must be 0–65535.");
          return;
        }
        const built =
          mode === "classic" ? emptyClassicPrl(id) : emptyExtendedPrl(id);
        body = { name: name.trim(), notes, built };
      }
      const res = await fetch("/api/prls", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      const data = await res.json();
      if (!res.ok || data.error) {
        setError(data.error ?? "save failed");
        return;
      }
      const newId = data.prl?.summary?.prlId;
      router.push(newId ? `/prls/${newId}` : "/prls");
    } catch (err) {
      setError(err instanceof Error ? err.message : "save failed");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <h1 className="text-primary text-lg font-medium">New PRL</h1>

      <div className="flex gap-1 border border-border rounded text-xs w-fit">
        <ModeTab id="upload" current={mode} onClick={setMode}>
          Upload .prl
        </ModeTab>
        <ModeTab id="classic" current={mode} onClick={setMode}>
          Build classic
        </ModeTab>
        <ModeTab id="extended" current={mode} onClick={setMode}>
          Build extended
        </ModeTab>
      </div>

      <Card title={modeLabel(mode)}>
        <form onSubmit={submit} className="space-y-3 text-xs">
          <label className="block">
            <span className="text-muted">Name</span>
            <input
              className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Verizon 2024 default"
              maxLength={120}
            />
          </label>
          <label className="block">
            <span className="text-muted">Notes (optional)</span>
            <textarea
              className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1 font-mono"
              rows={2}
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
            />
          </label>

          {mode === "upload" && (
            <label className="block">
              <span className="text-muted">PRL file</span>
              <input
                type="file"
                accept=".prl,application/octet-stream"
                className="block w-full mt-1 text-primary"
                onChange={(e) => setFile(e.target.files?.[0] ?? null)}
              />
            </label>
          )}

          {(mode === "classic" || mode === "extended") && (
            <label className="block">
              <span className="text-muted">Initial PR_LIST_ID (0–65535)</span>
              <input
                type="number"
                className="block w-full mt-1 bg-bg border border-border rounded px-2 py-1 font-mono"
                min={0}
                max={0xffff}
                value={prListId}
                onChange={(e) => setPrListId(e.target.value)}
                placeholder="e.g. 1"
              />
            </label>
          )}

          {error && <p className="text-accent-red">{error}</p>}

          <div className="flex gap-2">
            <button
              type="submit"
              disabled={busy}
              className="bg-accent-blue/20 border border-accent-blue/40 text-accent-blue px-3 py-1 rounded hover:bg-accent-blue/30 disabled:opacity-50"
            >
              {busy ? "Creating…" : mode === "upload" ? "Upload" : "Create empty PRL"}
            </button>
            <button
              type="button"
              onClick={() => router.push("/prls")}
              className="text-muted px-3 py-1 hover:text-primary"
            >
              Cancel
            </button>
          </div>
        </form>
      </Card>
    </div>
  );
}

function ModeTab({
  id,
  current,
  onClick,
  children,
}: {
  id: Mode;
  current: Mode;
  onClick: (m: Mode) => void;
  children: React.ReactNode;
}) {
  const active = id === current;
  return (
    <button
      type="button"
      onClick={() => onClick(id)}
      className={`px-3 py-1 ${
        active ? "bg-accent-blue/20 text-accent-blue" : "text-muted hover:text-primary"
      }`}
    >
      {children}
    </button>
  );
}

function modeLabel(m: Mode): string {
  switch (m) {
    case "upload": return "Upload an existing PRL";
    case "classic": return "Build a new classic PRL (SSPR_P_REV = 1)";
    case "extended": return "Build a new extended PRL (SSPR_P_REV = 3)";
  }
}
