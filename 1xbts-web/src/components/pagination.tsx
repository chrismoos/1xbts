"use client";

import { useId } from "react";

export interface PaginationProps {
  total: number;
  offset: number;
  limit: number;
  pageSizeOptions?: number[];
  onPageChange(nextOffset: number): void;
  onLimitChange(nextLimit: number): void;
}

const DEFAULT_PAGE_SIZES = [10, 25, 50, 100];

export function Pagination({
  total,
  offset,
  limit,
  pageSizeOptions = DEFAULT_PAGE_SIZES,
  onPageChange,
  onLimitChange,
}: PaginationProps) {
  const pageSizeSelectId = useId();
  const safeLimit = Math.max(limit, 1);
  const start = total === 0 ? 0 : offset + 1;
  const end = Math.min(offset + safeLimit, total);
  const canPrev = offset > 0;
  const canNext = end < total;

  return (
    <div className="flex items-center justify-between gap-3 text-xs text-muted py-2">
      <div className="flex items-center gap-2">
        <label htmlFor={pageSizeSelectId} className="text-dimmed">
          Show
        </label>
        <select
          id={pageSizeSelectId}
          className="bg-bg border border-border rounded px-1.5 py-0.5 text-xs"
          value={safeLimit}
          onChange={(e) => onLimitChange(Number(e.target.value))}
        >
          {pageSizeOptions.map((size) => (
            <option key={size} value={size}>
              {size}
            </option>
          ))}
        </select>
        <span className="text-dimmed">per page</span>
      </div>
      <div className="flex items-center gap-3">
        <span className="font-mono text-dimmed">
          {total === 0 ? "0 of 0" : `${start}–${end} of ${total}`}
        </span>
        <button
          className="text-accent-blue hover:underline disabled:opacity-30 disabled:no-underline"
          disabled={!canPrev}
          onClick={() => onPageChange(Math.max(0, offset - safeLimit))}
        >
          ← Prev
        </button>
        <button
          className="text-accent-blue hover:underline disabled:opacity-30 disabled:no-underline"
          disabled={!canNext}
          onClick={() => onPageChange(offset + safeLimit)}
        >
          Next →
        </button>
      </div>
    </div>
  );
}
