const NEIGHBOR_PAGES = 1;

function pageItems(page: number, pages: number): (number | "gap")[] {
  const items: (number | "gap")[] = [];
  for (let p = 0; p < pages; p++) {
    if (p === 0 || p === pages - 1 || Math.abs(p - page) <= NEIGHBOR_PAGES) items.push(p);
    else if (items[items.length - 1] !== "gap") items.push("gap");
  }
  return items;
}

export function Pager({
  page,
  pageSize,
  total,
  pageSizes,
  onPage,
  onPageSize,
}: {
  page: number;
  pageSize: number;
  total: number;
  pageSizes: readonly number[];
  onPage: (page: number) => void;
  onPageSize: (size: number) => void;
}) {
  const pages = Math.max(1, Math.ceil(total / pageSize));
  const first = page * pageSize;
  const button =
    "min-w-7 h-7 px-1.5 rounded border border-border-input bg-surface-raised text-secondary text-xs disabled:text-dimmed disabled:cursor-default";
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 mt-3 text-xs text-muted">
      <span className="font-mono">
        {total === 0 ? "0" : `${first + 1}–${Math.min(total, first + pageSize)}`} of {total}
      </span>
      <div className="flex items-center gap-1">
        <button
          type="button"
          className={button}
          disabled={page === 0}
          onClick={() => onPage(page - 1)}
          aria-label="Previous page"
        >
          ‹
        </button>
        {pageItems(page, pages).map((item, i) =>
          item === "gap" ? (
            <span key={`gap-${i}`} className="px-1 text-dimmed">…</span>
          ) : (
            <button
              key={item}
              type="button"
              onClick={() => onPage(item)}
              className={`${button} ${
                item === page ? "border-accent-green/30 bg-accent-green-bg text-accent-green" : ""
              }`}
              aria-current={item === page ? "page" : undefined}
            >
              {item + 1}
            </button>
          ),
        )}
        <button
          type="button"
          className={button}
          disabled={page >= pages - 1}
          onClick={() => onPage(page + 1)}
          aria-label="Next page"
        >
          ›
        </button>
      </div>
      <label className="flex items-center gap-1">
        Rows
        <select
          value={pageSize}
          onChange={(e) => onPageSize(Number(e.target.value))}
          className="bg-surface-raised border border-border-input rounded px-1 py-0.5 text-secondary"
        >
          {pageSizes.map((size) => (
            <option key={size} value={size}>
              {size}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}
