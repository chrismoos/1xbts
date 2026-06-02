"use client";

import { useState } from "react";
import { generateImsi } from "@/lib/imsi-generate";

interface Props {
  phoneNumber: string;
  onGenerated: (imsi: string) => void;
}

export function ImsiGenerateButton({ phoneNumber, onGenerated }: Props) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const click = async () => {
    setErr(null);
    setBusy(true);
    try {
      const res = await fetch("/api/cell-identity");
      if (!res.ok) {
        setErr("could not load cell identity");
        return;
      }
      const { mcc, imsi1112 } = (await res.json()) as {
        mcc: string;
        imsi1112: string;
      };
      const { imsi, error } = generateImsi(phoneNumber, mcc, imsi1112);
      if (error) {
        setErr(error);
        return;
      }
      onGenerated(imsi);
    } catch {
      setErr("could not load cell identity");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex items-center gap-1">
        <button
          type="button"
          onClick={click}
          disabled={busy || !phoneNumber.trim()}
          className="rounded border border-border-input px-2 py-0.5 text-[11px] text-secondary hover:bg-surface-raised disabled:opacity-50"
        >
          {busy ? "…" : "Generate"}
        </button>
        <span
          title="Builds the 15-digit IMSI by concatenating the cell's MCC, IMSI_11_12, and a 10-digit IMSI_S derived from the phone number (left-padded with zeros if shorter, last 10 digits taken if longer)."
          className="cursor-help text-[11px] text-muted"
        >
          ⓘ
        </span>
      </div>
      {err && <p className="text-[11px] text-accent-red">{err}</p>}
    </div>
  );
}
