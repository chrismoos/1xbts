"use client";

import { useState } from "react";
import { useLive } from "@/lib/live";
import type { BtsSummary } from "@/lib/proto/bts_management/v1/service";

import { defaultCellToken } from "@/lib/cell";
import type { ApiErrorBody } from "./api-error-body";

// A cell plus the base station it belongs to, so callers can address it across
// base stations (a cell/sector token is only unique within one base station).
export type CellEntry = BtsSummary & { baseStation: string };

interface BtsListState {
  cells: CellEntry[];
  error: ApiErrorBody | string | null;
  loading: boolean;
}

export function useBtsList(): BtsListState {
  const { data, error, errorBody, loading } = useLive<{ bts?: CellEntry[] }>("/api/bts");
  return { cells: data?.bts ?? NO_CELLS, error: errorBody ?? error, loading };
}

const NO_CELLS: CellEntry[] = [];

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
