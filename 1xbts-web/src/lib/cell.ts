import {
  BtsAttachState,
  type BtsSummary,
  type CellRequest,
} from "@/lib/proto/bts_management/v1/service";
import type { CellId } from "@/lib/proto/bsc/v1/service";
import type { BsSelector } from "@/lib/proto/mgmt/v1/service";

// A cell picker option packs the cell token and its base station, since a
// token is only unique within one base station.
const CELL_OPTION_SEP = "@@";

export function cellOptionValue(token: string, baseStation: string): string {
  return `${token}${CELL_OPTION_SEP}${baseStation}`;
}

export function parseCellOption(value: string): {
  token: string;
  baseStation: string;
} {
  const [token = "", baseStation = ""] = value.split(CELL_OPTION_SEP);
  return { token, baseStation };
}

// Base station endpoint without its scheme, for compact picker labels.
export function shortBaseStation(endpoint: string): string {
  return endpoint.replace(/^https?:\/\//, "");
}

// A cell is addressed in URLs as "<cell>:<sector>" via the `cell` query
// parameter on the BTS management API routes. The per-peer detail route is
// keyed by peer id instead (see peerHref).
export function cellToken(cell: CellId | null | undefined): string | null {
  if (!cell) return null;
  return `${cell.cell}:${cell.sector}`;
}

export function parseCellToken(
  token: string | null | undefined,
): CellId | undefined {
  if (!token) return undefined;
  const [cellPart, sectorPart] = token.split(":");
  const cell = Number(cellPart);
  const sector = Number(sectorPart);
  if (!Number.isInteger(cell) || !Number.isInteger(sector)) return undefined;
  return { cell, sector };
}

// Builds the CellRequest for an API route. An absent or unparsable `cell`
// parameter leaves the cell unset, which the backend resolves to the single
// enrolled cell.
export function cellRequestFromUrl(url: string): { cell?: CellId } {
  const raw = new URL(url).searchParams.get("cell");
  return { cell: parseCellToken(raw) };
}

// A management CellRequest from a route's URL: a `peerId` query param, or a
// `cell` param for the callers that still address by cell token.
export function peerRequestFromUrl(url: string): {
  peerId?: string;
  cell?: CellId;
} {
  const params = new URL(url).searchParams;
  const peerId = params.get("peerId");
  if (peerId) return { peerId };
  return { cell: parseCellToken(params.get("cell")) };
}

// Base-station selector from a route's URL. An absent `baseStation` param
// yields an empty selector, which the MSC resolves to the sole base station.
export function selectorFromUrl(url: string): BsSelector {
  return { baseStation: new URL(url).searchParams.get("baseStation") ?? "" };
}

// A base-station-scoped NetworkManagement request from a route's URL.
export function bsRequestFromUrl(url: string): { selector: BsSelector } {
  return { selector: selectorFromUrl(url) };
}

// A cell-scoped NetworkManagement request from a route's URL, pairing the
// base-station selector with the peer/cell CellRequest the BTS routes parse.
export function cellScopedRequestFromUrl(url: string): {
  selector: BsSelector;
  request: CellRequest;
} {
  return { selector: selectorFromUrl(url), request: peerRequestFromUrl(url) };
}

// Cell a page addresses when the URL names none. An in-service cell wins over
// a merely enrolled one so the default lands on a cell that can answer.
export function defaultCellToken(cells: BtsSummary[]): string | null {
  const inService = cells.find(
    (summary) => summary.state === BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE,
  );
  return cellToken((inService ?? cells[0])?.cell);
}

export function cellLabel(cell: CellId | null | undefined): string {
  return cell ? `${cell.cell}/${cell.sector}` : "-";
}

export function cellTitle(cell: CellId | null | undefined): string {
  return cell ? `Cell ${cell.cell} · Sector ${cell.sector}` : "Cell";
}

export function sameCell(
  a: CellId | null | undefined,
  b: CellId | null | undefined,
): boolean {
  return !!a && !!b && a.cell === b.cell && a.sector === b.sector;
}

export function cellHref(
  path: string,
  token: string | null | undefined,
  baseStation?: string | null,
): string {
  if (!token) return path;
  const params = new URLSearchParams({ cell: token });
  if (baseStation) params.set("baseStation", baseStation);
  return `${path}?${params.toString()}`;
}

// Addresses a management API route by the peer's stable opaque id.
export function peerHref(
  path: string,
  peerId: string | null | undefined,
  baseStation?: string | null,
): string {
  if (!peerId) return path;
  const params = new URLSearchParams({ peerId });
  if (baseStation) params.set("baseStation", baseStation);
  return `${path}?${params.toString()}`;
}

// Cell whose HRPD sector advertises `colorCode`. The color code is the top
// byte of an access terminal's on-air identifier, so it maps an HRPD session
// back to the cell that issued its UATI. The BSC keeps color codes distinct
// across cells, so at most one matches.
export function cellForColorCode(
  cells: BtsSummary[],
  colorCode: number | null | undefined,
): CellId | undefined {
  if (colorCode == null) return undefined;
  return cells.find((cell) => cell.evdoColorCode === colorCode)?.cell;
}

// Peer id of the in-service cell matching `cell`, for linking to its detail
// page.
export function peerIdForCell(
  cells: BtsSummary[],
  cell: CellId | null | undefined,
): string | undefined {
  if (!cell) return undefined;
  return cells.find((summary) => sameCell(summary.cell, cell))?.peerId;
}

export function attachStateLabel(state: BtsAttachState): string {
  switch (state) {
    case BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE:
      return "In Service";
    case BtsAttachState.BTS_ATTACH_STATE_ENROLLED:
      return "Enrolled";
    case BtsAttachState.BTS_ATTACH_STATE_DISCONNECTED:
      return "Disconnected";
    default:
      return "Unknown";
  }
}

export function attachStateBadgeClass(state: BtsAttachState): string {
  switch (state) {
    case BtsAttachState.BTS_ATTACH_STATE_IN_SERVICE:
      return "bg-badge-green-bg text-badge-green-text";
    case BtsAttachState.BTS_ATTACH_STATE_ENROLLED:
      return "bg-badge-yellow-bg text-badge-yellow-text";
    case BtsAttachState.BTS_ATTACH_STATE_DISCONNECTED:
      return "bg-badge-red-bg text-badge-red-text";
    default:
      return "bg-surface-raised text-muted";
  }
}
