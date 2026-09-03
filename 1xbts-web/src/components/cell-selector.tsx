"use client";

import Link from "next/link";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import {
  attachStateLabel,
  cellHref,
  cellOptionValue,
  cellToken,
  parseCellOption,
  shortBaseStation,
} from "@/lib/cell";
import { useBtsList } from "@/lib/use-bts-list";

// Cell picker for the pages that render one cell at a time. Spans every base
// station and threads the base station alongside the cell so the selection is
// unambiguous. Renders nothing when a single cell is enrolled.
export function CellSelector({ selected }: { selected?: string | null }) {
  const { cells } = useBtsList();
  const router = useRouter();
  const pathname = usePathname();
  const baseStation = useSearchParams().get("baseStation") ?? "";

  const withCell = cells.filter((cell) => cell.cell);
  if (withCell.length <= 1) return null;
  const multiBs = new Set(withCell.map((cell) => cell.baseStation)).size > 1;

  const current = withCell.find(
    (cell) =>
      cellToken(cell.cell) === selected &&
      (!baseStation || cell.baseStation === baseStation),
  );
  const value = current
    ? cellOptionValue(cellToken(current.cell) ?? "", current.baseStation)
    : "";

  return (
    <div className="flex items-center gap-2">
      <label
        htmlFor="cell-selector"
        className="text-muted text-xs uppercase tracking-wide"
      >
        Cell
      </label>
      <select
        id="cell-selector"
        value={value}
        onChange={(event) => {
          const { token, baseStation: bs } = parseCellOption(event.target.value);
          router.push(cellHref(pathname, token, bs));
        }}
        className="glass-input text-sm"
      >
        {!value && <option value="">Select a cell</option>}
        {withCell.map((cell) => {
          const token = cellToken(cell.cell) ?? "";
          const prefix = multiBs ? `${shortBaseStation(cell.baseStation)} ` : "";
          return (
            <option
              key={cellOptionValue(token, cell.baseStation)}
              value={cellOptionValue(token, cell.baseStation)}
            >
              {`${prefix}${token} · ${attachStateLabel(cell.state)}`}
            </option>
          );
        })}
      </select>
      {current?.peerId && (
        <Link
          href={`/bts/${encodeURIComponent(current.peerId)}?baseStation=${encodeURIComponent(current.baseStation)}`}
          className="text-xs text-accent-green hover:text-accent-green transition-colors"
        >
          Cell detail &rarr;
        </Link>
      )}
    </div>
  );
}
