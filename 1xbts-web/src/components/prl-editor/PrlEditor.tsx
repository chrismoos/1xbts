"use client";

import { useEffect, useReducer, useState, useMemo } from "react";
import { Card } from "@/components/card";
import {
  PrlDecoded,
} from "@/lib/proto/hlr/v1/service";
import { initialState, isDirty, modeOf, reducer } from "./state";
import { validate } from "./validation";
import { HeaderTab } from "./HeaderTab";
import { AcqTab } from "./AcqTab";
import { SysTab } from "./SysTab";
import { CommonSubnetTab } from "./CommonSubnetTab";

type Tab = "header" | "acq" | "sys" | "subnet";

export function PrlEditor({
  decoded,
  onSave,
  saving,
  error,
}: {
  decoded: PrlDecoded;
  onSave: (next: PrlDecoded) => Promise<void> | void;
  saving?: boolean;
  error?: string;
}) {
  const [state, dispatch] = useReducer(reducer, decoded, initialState);
  const [tab, setTab] = useState<Tab>("header");

  // Reset editor state if the parent reloads with new content (e.g. after a save).
  useEffect(() => {
    dispatch({ type: "load", payload: decoded });
  }, [decoded]);

  const mode = modeOf(state.draft);
  const dirty = isDirty(state);
  const validation = useMemo(() => validate(state.draft), [state.draft]);

  // Warn on unsaved navigation.
  useEffect(() => {
    if (!dirty) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      return "Unsaved PRL changes — leave anyway?";
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [dirty]);

  const save = async () => {
    if (!validation.isValid || !dirty) return;
    await onSave(state.draft);
  };

  const tabs: { id: Tab; label: string; visible: boolean }[] = [
    { id: "header", label: "Header", visible: true },
    {
      id: "acq",
      label: `ACQ_TABLE (${acqCount(state.draft)})`,
      visible: true,
    },
    {
      id: "sys",
      label: `SYS_TABLE (${sysCount(state.draft)})`,
      visible: true,
    },
    {
      id: "subnet",
      label: `Common Subnet (${subnetCount(state.draft)})`,
      visible: mode === "extended",
    },
  ];

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3 flex-wrap">
        <div className="flex gap-1 border border-border rounded">
          {tabs
            .filter((t) => t.visible)
            .map((t) => (
              <button
                key={t.id}
                onClick={() => setTab(t.id)}
                className={`px-3 py-1 text-xs ${
                  tab === t.id
                    ? "bg-accent-blue/20 text-accent-blue"
                    : "text-muted hover:text-primary"
                }`}
              >
                {t.label}
              </button>
            ))}
        </div>
        <div className="ml-auto flex items-center gap-2 text-xs">
          {dirty && (
            <span className="text-accent-orange">● Unsaved changes</span>
          )}
          {!validation.isValid && (
            <span
              className="text-accent-red cursor-pointer"
              title={[...validation.errors.entries()]
                .slice(0, 5)
                .map(([k, v]) => `${k}: ${v}`)
                .join("\n")}
            >
              {validation.errors.size} validation issue
              {validation.errors.size === 1 ? "" : "s"}
            </span>
          )}
          <button
            onClick={save}
            disabled={!dirty || !validation.isValid || saving}
            className="bg-accent-blue/20 border border-accent-blue/40 text-accent-blue px-3 py-1 rounded disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {saving ? "Saving…" : "Save PRL"}
          </button>
        </div>
      </div>

      {error && (
        <Card title="Save error" className="border-accent-red/40">
          <p className="text-accent-red text-xs">{error}</p>
        </Card>
      )}

      {tab === "header" && (
        <HeaderTab state={state} dispatch={dispatch} errors={validation.errors} />
      )}
      {tab === "acq" && (
        <AcqTab state={state} dispatch={dispatch} errors={validation.errors} />
      )}
      {tab === "sys" && (
        <SysTab state={state} dispatch={dispatch} errors={validation.errors} />
      )}
      {tab === "subnet" && mode === "extended" && (
        <CommonSubnetTab
          state={state}
          dispatch={dispatch}
          errors={validation.errors}
        />
      )}
    </div>
  );
}

function acqCount(d: PrlDecoded): number {
  return (d.classic ?? d.extended)?.acquisitionRecords.length ?? 0;
}
function sysCount(d: PrlDecoded): number {
  return (d.classic ?? d.extended)?.systemRecords.length ?? 0;
}
function subnetCount(d: PrlDecoded): number {
  return d.extended?.commonSubnetRecords.length ?? 0;
}
