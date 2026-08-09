"use client";

import { useEffect, useReducer, useState, useMemo } from "react";
import Link from "next/link";
import { Card } from "@/components/card";
import {
  PrlDecoded,
} from "@/lib/proto/hlr/v1/service";
import { initialState, isDirty, modeOf, reducer } from "./state";
import { validate } from "./validation";
import { HeaderTab, PrlGeneralMetadata } from "./HeaderTab";
import { AcqTab } from "./AcqTab";
import { SysTab } from "./SysTab";
import { CommonSubnetTab } from "./CommonSubnetTab";
import { SaveReviewModal } from "./SaveReviewModal";
import { buildSaveReview, type SaveReview } from "./save-review";

type Tab = "header" | "acq" | "sys" | "subnet";

function rowIndex(raw: string | null): number | undefined {
  return raw != null && /^\d+$/.test(raw) ? Number(raw) : undefined;
}

function clearFiltersFor(url: URL, tab: "acq" | "sys" | "subnet") {
  for (const key of [...url.searchParams.keys()]) {
    if (key.startsWith(tab)) url.searchParams.delete(key);
  }
}

export function PrlEditor({
  decoded,
  onSave,
  saving,
  error,
  metadata,
  metadataDirty,
}: {
  decoded: PrlDecoded;
  onSave: (next: PrlDecoded) => Promise<void> | void;
  saving?: boolean;
  error?: string;
  metadata: PrlGeneralMetadata;
  metadataDirty: boolean;
}) {
  const [state, dispatch] = useReducer(reducer, decoded, initialState);
  const [tab, setTab] = useState<Tab>("header");
  const [acqNavigation, setAcqNavigation] = useState<{
    index: number;
    token: number;
  }>();
  const [sysNavigation, setSysNavigation] = useState<{
    index: number;
    token: number;
  }>();
  const [pendingReview, setPendingReview] = useState<SaveReview | null>(null);
  const mode = modeOf(state.draft);

  // Reset editor state if the parent reloads with new content (e.g. after a save).
  useEffect(() => {
    dispatch({ type: "load", payload: decoded });
  }, [decoded]);

  useEffect(() => {
    const navigateFromUrl = () => {
      const url = new URL(window.location.href);
      const acqMatch = url.hash.match(/^#acq-(\d+)$/);
      if (acqMatch) {
        const index = Number(acqMatch[1]);
        clearFiltersFor(url, "acq");
        url.searchParams.set("tab", "acq");
        url.searchParams.set("acqRow", String(index));
        url.searchParams.set("acqOpen", String(index));
        url.hash = "";
        window.history.replaceState(window.history.state, "", url);
        setAcqNavigation((current) => ({
          index,
          token: (current?.token ?? 0) + 1,
        }));
        setSysNavigation(undefined);
        setTab("acq");
        return;
      }
      const sysMatch = url.hash.match(/^#sys-(\d+)$/);
      if (sysMatch) {
        const index = Number(sysMatch[1]);
        clearFiltersFor(url, "sys");
        url.searchParams.set("tab", "sys");
        url.searchParams.set("sysRow", String(index));
        url.searchParams.set("sysOpen", String(index));
        url.hash = "";
        window.history.replaceState(window.history.state, "", url);
        setSysNavigation((current) => ({
          index,
          token: (current?.token ?? 0) + 1,
        }));
        setAcqNavigation(undefined);
        setTab("sys");
        return;
      }

      const requested = url.searchParams.get("tab");
      const nextTab: Tab =
        requested === "acq" ||
        requested === "sys" ||
        (requested === "subnet" && mode === "extended")
          ? requested
          : "header";
      const acqIndex =
        nextTab === "acq" ? rowIndex(url.searchParams.get("acqRow")) : undefined;
      const sysIndex =
        nextTab === "sys" ? rowIndex(url.searchParams.get("sysRow")) : undefined;
      setAcqNavigation((current) =>
        acqIndex == null
          ? undefined
          : { index: acqIndex, token: (current?.token ?? 0) + 1 },
      );
      setSysNavigation((current) =>
        sysIndex == null
          ? undefined
          : { index: sysIndex, token: (current?.token ?? 0) + 1 },
      );
      setTab(nextTab);
    };
    navigateFromUrl();
    window.addEventListener("hashchange", navigateFromUrl);
    window.addEventListener("popstate", navigateFromUrl);
    return () => {
      window.removeEventListener("hashchange", navigateFromUrl);
      window.removeEventListener("popstate", navigateFromUrl);
    };
  }, [mode]);

  const dirty = isDirty(state);
  const hasUnsavedChanges = dirty || metadataDirty;
  const metadataValid = metadata.name.trim() !== "";
  const validation = useMemo(() => validate(state.draft), [state.draft]);

  // Warn on unsaved navigation.
  useEffect(() => {
    if (!hasUnsavedChanges) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      return "Unsaved PRL changes — leave anyway?";
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [hasUnsavedChanges]);

  const save = () => {
    if (!validation.isValid || !metadataValid || !hasUnsavedChanges) return;
    setPendingReview(buildSaveReview(state, metadata));
  };

  const confirmSave = async () => {
    await onSave(state.draft);
    setPendingReview(null);
  };

  const navigateToRecord = (target: "acq" | "sys", index: number) => {
    const url = new URL(window.location.href);
    clearFiltersFor(url, target);
    url.searchParams.set("tab", target);
    url.searchParams.set(`${target}Row`, String(index));
    url.searchParams.set(`${target}Open`, String(index));
    url.hash = "";
    window.history.pushState(window.history.state, "", url);
  };

  const navigateToAcq = (index: number) => {
    navigateToRecord("acq", index);
    setAcqNavigation((current) => ({
      index,
      token: (current?.token ?? 0) + 1,
    }));
    setSysNavigation(undefined);
    setTab("acq");
  };

  const navigateToSys = (index: number) => {
    navigateToRecord("sys", index);
    setSysNavigation((current) => ({
      index,
      token: (current?.token ?? 0) + 1,
    }));
    setAcqNavigation(undefined);
    setTab("sys");
  };

  const selectTab = (nextTab: Tab) => {
    const url = new URL(window.location.href);
    url.searchParams.set("tab", nextTab === "header" ? "general" : nextTab);
    url.hash = "";
    if (url.href !== window.location.href) {
      window.history.pushState(window.history.state, "", url);
    }
    setAcqNavigation(undefined);
    setSysNavigation(undefined);
    setTab(nextTab);
  };

  const tabs: { id: Tab; label: string; visible: boolean }[] = [
    { id: "header", label: "General", visible: true },
    {
      id: "acq",
      label: `Acquisitions (${acqCount(state.draft)})`,
      visible: true,
    },
    {
      id: "sys",
      label: `Systems (${sysCount(state.draft)})`,
      visible: true,
    },
    {
      id: "subnet",
      label: `Common Subnets (${subnetCount(state.draft)})`,
      visible: mode === "extended",
    },
  ];

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3 flex-wrap">
        <div className="flex min-w-0 items-center gap-2 text-xs">
          <Link href="/prls" className="text-muted hover:text-primary">
            ← PRLs
          </Link>
          <span className="text-dimmed">/</span>
          <span className="max-w-48 truncate text-primary" title={metadata.name}>
            {metadata.name || "Untitled PRL"}
          </span>
          {metadata.isDefault && (
            <span className="text-accent-green">✓ default</span>
          )}
        </div>
        <div className="flex gap-1 border border-border rounded">
          {tabs
            .filter((t) => t.visible)
            .map((t) => (
              <button
                key={t.id}
                onClick={() => selectTab(t.id)}
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
          {hasUnsavedChanges && (
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
            disabled={
              !hasUnsavedChanges ||
              !validation.isValid ||
              !metadataValid ||
              saving
            }
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
        <HeaderTab
          state={state}
          dispatch={dispatch}
          errors={validation.errors}
          metadata={metadata}
        />
      )}
      {tab === "acq" && (
        <AcqTab
          key={acqNavigation?.token ?? "acq"}
          state={state}
          dispatch={dispatch}
          errors={validation.errors}
          focusedIndex={acqNavigation?.index}
          onNavigateSys={navigateToSys}
        />
      )}
      {tab === "sys" && (
        <SysTab
          key={sysNavigation?.token ?? "sys"}
          state={state}
          dispatch={dispatch}
          errors={validation.errors}
          onNavigateAcq={navigateToAcq}
          focusedIndex={sysNavigation?.index}
        />
      )}
      {tab === "subnet" && mode === "extended" && (
        <CommonSubnetTab
          state={state}
          dispatch={dispatch}
          errors={validation.errors}
          onNavigateSys={navigateToSys}
        />
      )}
      {pendingReview && (
        <SaveReviewModal
          review={pendingReview}
          saving={Boolean(saving)}
          onCancel={() => setPendingReview(null)}
          onConfirm={confirmSave}
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
