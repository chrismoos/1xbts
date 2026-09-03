import { getMscManagementClient, getNetworkManagementClient } from "./client";
import type { BsSelector } from "@/lib/proto/mgmt/v1/service";

export interface EvdoCellRef {
  selector: BsSelector;
  cell: { peerId: string };
}

// Every EV-DO cell across all base stations, for network-wide HRPD rollups.
export async function evdoCells(signal?: AbortSignal): Promise<EvdoCellRef[]> {
  const msc = getMscManagementClient();
  const net = getNetworkManagementClient();
  const { nodes } = await msc.listBaseStations({}, { signal });
  const refs: EvdoCellRef[] = [];
  for (const bs of nodes) {
    const selector: BsSelector = { baseStation: bs.managementEndpoint };
    try {
      const { bts } = await net.listBts({ selector }, { signal });
      for (const cell of bts) {
        if (cell.evdoEnabled && cell.peerId) {
          refs.push({ selector, cell: { peerId: cell.peerId } });
        }
      }
    } catch {
      // Skip a base station that is offline.
    }
  }
  return refs;
}

// True when a request names no base station, peer, or cell, so a network-wide
// aggregate is wanted rather than one cell.
export function isUnscoped(url: string): boolean {
  const params = new URL(url).searchParams;
  return !params.get("baseStation") && !params.get("peerId") && !params.get("cell");
}
