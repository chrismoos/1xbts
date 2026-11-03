"use client";

import { useLive } from "@/lib/live";
import type { BaseStationSummary } from "@/lib/proto/msc_management/v1/service";

interface BaseStationListState {
  nodes: BaseStationSummary[];
  error: string | null;
  loading: boolean;
}

export function useBaseStations(): BaseStationListState {
  const { data, error, loading } = useLive<{ nodes?: BaseStationSummary[] }>("/api/base-stations");
  return { nodes: data?.nodes ?? NO_NODES, error, loading };
}

const NO_NODES: BaseStationSummary[] = [];
