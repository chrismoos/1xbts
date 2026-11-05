"use client";

import { useLive } from "@/lib/live";
import type { BaseStationSummary } from "@/lib/proto/msc_management/v1/service";
import type { ApiErrorBody } from "./api-error-body";

interface BaseStationListState {
  nodes: BaseStationSummary[];
  error: ApiErrorBody | string | null;
  loading: boolean;
}

export function useBaseStations(): BaseStationListState {
  const { data, error, errorBody, loading } = useLive<{ nodes?: BaseStationSummary[] }>("/api/base-stations");
  return { nodes: data?.nodes ?? NO_NODES, error: errorBody ?? error, loading };
}

const NO_NODES: BaseStationSummary[] = [];
