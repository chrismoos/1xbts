"use client";

import { useState } from "react";
import { generateImsi } from "@/lib/imsi-generate";
import {
  attachStateLabel,
  cellHref,
  cellOptionValue,
  cellToken,
  parseCellOption,
  shortBaseStation,
} from "@/lib/cell";
import { useBtsList } from "@/lib/use-bts-list";

interface Props {
  phoneNumber: string;
  onGenerated: (imsi: string) => void;
}

export function ImsiGenerateButton({ phoneNumber, onGenerated }: Props) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const { cells } = useBtsList();
  // MCC and IMSI_11_12 are per cell, so the button names the cell (and its base
  // station) it read them from once more than one is enrolled.
  const withCell = cells.filter((c) => c.cell);
  const multiBs = new Set(withCell.map((c) => c.baseStation)).size > 1;
  const defaultValue = withCell[0]
    ? cellOptionValue(cellToken(withCell[0].cell) ?? "", withCell[0].baseStation)
    : "";
  const value = picked ?? defaultValue;
  const { token, baseStation } = parseCellOption(value);

  const click = async () => {
    setErr(null);
    setBusy(true);
    try {
      const res = await fetch(cellHref("/api/cell-identity", token, baseStation));
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
        {withCell.length > 1 && (
          <select
            aria-label="Cell"
            value={value}
            onChange={(event) => setPicked(event.target.value)}
            className="rounded border border-border-input bg-transparent px-1 py-0.5 text-[11px] text-secondary"
          >
            {withCell.map((summary) => {
              const cellTok = cellToken(summary.cell) ?? "";
              const prefix = multiBs ? `${shortBaseStation(summary.baseStation)} ` : "";
              return (
                <option
                  key={cellOptionValue(cellTok, summary.baseStation)}
                  value={cellOptionValue(cellTok, summary.baseStation)}
                >
                  {`${prefix}${cellTok} · ${attachStateLabel(summary.state)}`}
                </option>
              );
            })}
          </select>
        )}
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
