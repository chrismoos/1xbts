"use client";

import { useEffect, useState } from "react";
import type { BtsSummary } from "@/lib/proto/bts_management/v1/service";

import { defaultCellToken } from "@/lib/cell";

// A cell plus the base station it belongs to, so callers can address it across
// base stations (a cell/sector token is only unique within one base station).
export type CellEntry = BtsSummary & { baseStation: string };

interface BtsListState {
  cells: CellEntry[];
  error: string | null;
  loading: boolean;
}

// Polls every configured peer, including offline ones with no cell yet.
// `pollMs` of 0 fetches once.
export function useBtsList(pollMs = 5000): BtsListState {
  const [state, setState] = useState<BtsListState>({
    cells: [],
    error: null,
    loading: true,
  });

  useEffect(() => {
    let cancelled = false;

    const load = () => {
      fetch("/api/bts", { cache: "no-store" })
        .then((r) => r.json())
        .then((data: { bts?: CellEntry[]; error?: string }) => {
          if (cancelled) return;
          if (data.error) {
            setState({ cells: [], error: data.error, loading: false });
          } else {
            setState({ cells: data.bts ?? [], error: null, loading: false });
          }
        })
        .catch((err) => {
          if (cancelled) return;
          setState({
            cells: [],
            error: err instanceof Error ? err.message : "unknown error",
            loading: false,
          });
        });
    };

    load();
    if (pollMs <= 0) return () => { cancelled = true; };
    const interval = setInterval(load, pollMs);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [pollMs]);

  return state;
}

// Default cell for a page whose URL names none, pinned to the first cell it
// resolves. The list refreshes every few seconds, so an unpinned default
// would silently retarget the page when a cell drops out of service.
export function usePinnedDefaultCell(cells: BtsSummary[]): string | null {
  const [pinned, setPinned] = useState<string | null>(null);
  const fallback = defaultCellToken(cells);
  if (pinned === null && fallback !== null) {
    // Adjusting state during render is the React-sanctioned way to latch a
    // value derived from props exactly once.
    setPinned(fallback);
  }
  return pinned ?? fallback;
}
