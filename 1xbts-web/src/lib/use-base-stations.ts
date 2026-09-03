"use client";

import { useEffect, useState } from "react";
import type { BaseStationSummary } from "@/lib/proto/msc_management/v1/service";

interface BaseStationListState {
  nodes: BaseStationSummary[];
  error: string | null;
  loading: boolean;
}

// Polls the base stations the MSC serves. `pollMs` of 0 fetches once.
export function useBaseStations(pollMs = 5000): BaseStationListState {
  const [state, setState] = useState<BaseStationListState>({
    nodes: [],
    error: null,
    loading: true,
  });

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const response = await fetch("/api/base-stations");
        const data = (await response.json()) as {
          nodes?: BaseStationSummary[];
          error?: string;
        };
        if (cancelled) return;
        if (data.error) {
          setState({ nodes: [], error: data.error, loading: false });
        } else {
          setState({ nodes: data.nodes ?? [], error: null, loading: false });
        }
      } catch (err) {
        if (cancelled) return;
        const msg = err instanceof Error ? err.message : "unknown error";
        setState({ nodes: [], error: msg, loading: false });
      }
    };
    void load();
    if (pollMs <= 0) return () => {
      cancelled = true;
    };
    const timer = setInterval(() => void load(), pollMs);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [pollMs]);

  return state;
}
