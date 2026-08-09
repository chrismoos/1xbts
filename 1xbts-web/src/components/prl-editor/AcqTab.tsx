import { Dispatch, useMemo, useRef } from "react";
import { Card } from "@/components/card";
import { Virtuoso, VirtuosoHandle } from "react-virtuoso";
import { v4 as uuid } from "@/lib/uuid-lite";
import {
  EditorState,
  EditorAction,
  modeOf,
  acqRecordsOf,
  sysRecordsOf,
} from "./state";
import { ErrorMap } from "./validation";
import { emptyClassicAcq, emptyExtAcq } from "./builders";
import {
  CLASSIC_ACQ_TYPE_OPTIONS,
  EXTENDED_ACQ_TYPE_OPTIONS,
  PCS_BLOCK_OPTIONS,
} from "@/lib/prl-options";
import { AcqRowEditor } from "./acq/AcqRowEditor";
import {
  acqBandClasses,
  acqChannels,
  acqDetailSummary,
  acqPcsBlocks,
  acqShortLabel,
} from "./acq-label";
import { systemReferenceLabel } from "./system-label";
import {
  compareSortValues,
  FilterField,
  FILTER_INPUT_CLASS,
  SortableColumnHeader,
  SortDirection,
  TableFilters,
} from "./shared/TableFilters";
import { SearchableSelect } from "./shared/SearchableSelect";
import { SearchableMultiSelect } from "./shared/SearchableMultiSelect";
import {
  remapOpenRowsAfterMove,
  remapOpenRowsAfterRemove,
  toggleOpenRow,
  useUrlSortState,
  useUrlStringListState,
  useUrlStringState,
} from "./shared/useUrlTableState";
import {
  DragHandle,
  SortableDragHandleProps,
  SortableList,
  SortableRow,
} from "./shared/SortableList";

type AcqSortKey = "index" | "type" | "details" | "references";
const ACQ_SORT_KEYS: readonly AcqSortKey[] = [
  "index",
  "type",
  "details",
  "references",
];

export function AcqTab({
  state,
  dispatch,
  errors,
  focusedIndex,
  onNavigateSys,
}: {
  state: EditorState;
  dispatch: Dispatch<EditorAction>;
  errors: ErrorMap;
  focusedIndex?: number;
  onNavigateSys: (index: number) => void;
}) {
  const mode = modeOf(state.draft);
  const records = acqRecordsOf(state.draft);
  const systemRecords = sysRecordsOf(state.draft);
  const [indexFilter, setIndexFilter] = useUrlStringState(
    "acqRow",
    focusedIndex == null ? "" : String(focusedIndex),
  );
  const [typeFilters, setTypeFilters] = useUrlStringListState("acqType");
  const [bandFilters, setBandFilters] = useUrlStringListState("acqBand");
  const [channelFilter, setChannelFilter] = useUrlStringState("acqChannel");
  const [blockFilters, setBlockFilters] = useUrlStringListState("acqBlock");
  const [referenceFilter, setReferenceFilter] = useUrlStringState(
    "acqReference",
    "all",
  );
  const [openRows, setOpenRows] = useUrlStringListState("acqOpen");
  const [sort, setSort] = useUrlSortState("acqSort", ACQ_SORT_KEYS, {
    key: "index",
    direction: "asc",
  });
  const listRef = useRef<VirtuosoHandle>(null);
  const options =
    mode === "extended" ? EXTENDED_ACQ_TYPE_OPTIONS : CLASSIC_ACQ_TYPE_OPTIONS;

  const rows = useMemo(
    () =>
      records.map((record, index) => {
        const references = systemRecords
          .map((systemRecord, systemIndex) => ({ systemRecord, systemIndex }))
          .filter(({ systemRecord }) => systemRecord.acqIndex === index);
        return {
          record,
          recordIndex: index,
          references,
          type: acqShortLabel(record, mode),
          details: acqDetailSummary(record),
          bands: acqBandClasses(record),
          channels: acqChannels(record),
          blocks: acqPcsBlocks(record),
        };
      }),
    [mode, records, systemRecords],
  );

  const typeOptions = useMemo(
    () =>
      [...new Map(rows.map((row) => [row.record.acqTypeRaw, row.type])).entries()]
        .sort(([left], [right]) => left - right),
    [rows],
  );
  const bandOptions = useMemo(
    () => [...new Set(rows.flatMap((row) => row.bands))].sort((a, b) => a - b),
    [rows],
  );
  const blockOptions = useMemo(
    () => [...new Set(rows.flatMap((row) => row.blocks))].sort((a, b) => a - b),
    [rows],
  );

  const filtered = useMemo(() => {
    return rows.filter((row) => {
      if (
        indexFilter !== "" &&
        row.recordIndex !== Number(indexFilter)
      ) {
        return false;
      }
      if (
        typeFilters.length > 0 &&
        !typeFilters.includes(String(row.record.acqTypeRaw))
      ) {
        return false;
      }
      if (
        bandFilters.length > 0 &&
        !row.bands.some((band) => bandFilters.includes(String(band)))
      ) {
        return false;
      }
      if (
        channelFilter !== "" &&
        !row.channels.includes(Number(channelFilter))
      ) {
        return false;
      }
      if (
        blockFilters.length > 0 &&
        !row.blocks.some((block) => blockFilters.includes(String(block)))
      ) {
        return false;
      }
      if (referenceFilter === "used" && row.references.length === 0) return false;
      if (referenceFilter === "unused" && row.references.length > 0) return false;
      if (
        referenceFilter.startsWith("sys:") &&
        !row.references.some(
          ({ systemIndex }) => systemIndex === Number(referenceFilter.slice(4)),
        )
      ) {
        return false;
      }
      return true;
    });
  }, [
    bandFilters,
    blockFilters,
    channelFilter,
    indexFilter,
    referenceFilter,
    rows,
    typeFilters,
  ]);

  const filtersActive =
    indexFilter !== "" ||
    typeFilters.length > 0 ||
    bandFilters.length > 0 ||
    channelFilter !== "" ||
    blockFilters.length > 0 ||
    referenceFilter !== "all";

  const visible = useMemo(() => {
    const next = [...filtered];
    next.sort((left, right) => {
      const leftValue =
        sort.key === "index"
          ? left.recordIndex
          : sort.key === "type"
            ? left.type
            : sort.key === "details"
              ? left.details
              : left.references.length;
      const rightValue =
        sort.key === "index"
          ? right.recordIndex
          : sort.key === "type"
            ? right.type
            : sort.key === "details"
              ? right.details
              : right.references.length;
      return compareSortValues(leftValue, rightValue, sort.direction);
    });
    return next;
  }, [filtered, sort]);

  const changeSort = (key: AcqSortKey) => {
    setSort((current) => ({
      key,
      direction:
        current.key === key && current.direction === "asc" ? "desc" : "asc",
    }));
  };

  const inPrlOrder = sort.key === "index" && sort.direction === "asc";

  const clearFilters = () => {
    setIndexFilter("");
    setTypeFilters([]);
    setBandFilters([]);
    setChannelFilter("");
    setBlockFilters([]);
    setReferenceFilter("all");
  };

  const addRow = (acqTypeRaw: number) => {
    const record =
      mode === "extended" ? emptyExtAcq(acqTypeRaw) : emptyClassicAcq(acqTypeRaw);
    const id = uuid();
    dispatch({ type: "addAcq", record, id });
    setOpenRows((current) => toggleOpenRow(current, records.length));
    setSort({ key: "index", direction: "asc" });
    clearFilters();
    requestAnimationFrame(() => {
      listRef.current?.scrollToIndex({
        index: records.length,
        align: "center",
        behavior: "smooth",
      });
    });
  };

  const toggleExpanded = (index: number) => {
    setOpenRows((current) => toggleOpenRow(current, index));
  };

  const moveRow = (from: number, to: number) => {
    setOpenRows((current) => remapOpenRowsAfterMove(current, from, to));
    dispatch({ type: "reorderAcq", from, to });
    setIndexFilter((current) =>
      current === String(from) ? String(to) : current,
    );
  };

  const removeRow = (index: number) => {
    setOpenRows((current) => remapOpenRowsAfterRemove(current, index));
    dispatch({ type: "removeAcq", index });
    setIndexFilter((current) =>
      current === String(index) ? "" : current,
    );
  };

  const navigateToSystemRecord = (index: number) => {
    onNavigateSys(index);
  };

  const referenceFilterDisplay =
    referenceFilter === "used"
      ? "Referenced"
      : referenceFilter === "unused"
        ? "Unreferenced"
        : referenceFilter.startsWith("sys:")
          ? `SYS #${referenceFilter.slice(4)}`
          : referenceFilter;

  return (
    <div className="grid items-start gap-3 xl:grid-cols-[15rem_minmax(0,1fr)]">
      <TableFilters
        title="Filter acquisitions"
        shown={filtered.length}
        total={rows.length}
        active={filtersActive}
        onClear={clearFilters}
        filters={[
          ...(indexFilter !== ""
            ? [
                {
                  label: "Row",
                  value: indexFilter,
                  onRemove: () => setIndexFilter(""),
                },
              ]
            : []),
          ...typeFilters.map((value) => ({
            label: "Type",
            value:
              typeOptions.find(([raw]) => raw === Number(value))?.[1] ?? value,
            onRemove: () =>
              setTypeFilters((current) =>
                current.filter((selected) => selected !== value),
              ),
          })),
          ...bandFilters.map((value) => ({
            label: "Band",
            value: `Band ${value}`,
            onRemove: () =>
              setBandFilters((current) =>
                current.filter((selected) => selected !== value),
              ),
          })),
          ...(channelFilter !== ""
            ? [
                {
                  label: "Channel",
                  value: channelFilter,
                  onRemove: () => setChannelFilter(""),
                },
              ]
            : []),
          ...blockFilters.map((value) => ({
            label: "PCS block",
            value:
              PCS_BLOCK_OPTIONS.find(
                (option) => option.value === Number(value),
              )?.label ?? value,
            onRemove: () =>
              setBlockFilters((current) =>
                current.filter((selected) => selected !== value),
              ),
          })),
          ...(referenceFilter !== "all"
            ? [
                {
                  label: "Referenced by",
                  value: referenceFilterDisplay,
                  onRemove: () => setReferenceFilter("all"),
                },
              ]
            : []),
        ]}
        quickFilters={[
          {
            label: "Referenced",
            active: referenceFilter === "used",
            onClick: () =>
              setReferenceFilter((current) =>
                current === "used" ? "all" : "used",
              ),
          },
          {
            label: "Unreferenced",
            active: referenceFilter === "unused",
            onClick: () =>
              setReferenceFilter((current) =>
                current === "unused" ? "all" : "unused",
              ),
          },
        ]}
      >
        <FilterField label="Row #" className="w-20">
          <input
            type="number"
            min={0}
            className={FILTER_INPUT_CLASS}
            value={indexFilter}
            onChange={(event) => setIndexFilter(event.target.value)}
            aria-label="Acquisition row index"
          />
        </FilterField>
        <FilterField label="Type" className="min-w-44">
          <SearchableMultiSelect
            values={typeFilters}
            options={typeOptions.map(([raw, label]) => ({
              value: String(raw),
              label,
            }))}
            onChange={setTypeFilters}
            placeholder="All types"
            ariaLabel="Acquisition types"
          />
        </FilterField>
        <FilterField label="Band class" className="min-w-28">
          <SearchableMultiSelect
            values={bandFilters}
            options={bandOptions.map((band) => ({
              value: String(band),
              label: `Band ${band}`,
            }))}
            onChange={setBandFilters}
            placeholder="All bands"
            ariaLabel="Band classes"
          />
        </FilterField>
        <FilterField label="Channel" className="w-24">
          <input
            type="number"
            min={0}
            className={FILTER_INPUT_CLASS}
            value={channelFilter}
            onChange={(event) => setChannelFilter(event.target.value)}
            aria-label="Exact channel number"
          />
        </FilterField>
        <FilterField label="PCS block" className="min-w-28">
          <SearchableMultiSelect
            values={blockFilters}
            options={blockOptions.map((block) => ({
              value: String(block),
              label:
                PCS_BLOCK_OPTIONS.find((option) => option.value === block)
                  ?.label ?? String(block),
            }))}
            onChange={setBlockFilters}
            placeholder="All blocks"
            ariaLabel="PCS blocks"
          />
        </FilterField>
        <FilterField label="Referenced by" className="min-w-52">
          <SearchableSelect
            value={referenceFilter}
            options={[
              { value: "all", label: "Any reference" },
              { value: "used", label: "Any referenced" },
              { value: "unused", label: "Unreferenced" },
              ...systemRecords.map((record, index) => ({
                value: `sys:${index}`,
                label: systemReferenceLabel(record, index),
                searchText: JSON.stringify(record),
              })),
            ]}
            onChange={setReferenceFilter}
            placeholder="Find a system record…"
            ariaLabel="Referencing system filter"
          />
        </FilterField>
      </TableFilters>

      <Card title={`Acquisition Records (${records.length})`}>
      <div className="flex items-center justify-end gap-2 mb-3 text-xs flex-wrap">
        <label className="text-muted">Add row:</label>
        <select
          className="bg-bg border border-border rounded px-2 py-1"
          defaultValue=""
          onChange={(e) => {
            if (e.target.value) {
              addRow(Number(e.target.value));
              e.target.value = "";
            }
          }}
        >
          <option value="">— Pick a type —</option>
          {options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </div>

      {records.length === 0 ? (
        <p className="text-dimmed text-xs">
          No acquisition records yet. Add one to get started — system records
          reference these by ACQ_INDEX.
        </p>
      ) : (
        <div className="border border-border rounded overflow-hidden">
          <AcqHeaderRow sort={sort} onSort={changeSort} />
          {filtered.length === 0 ? (
            <p className="text-dimmed text-xs px-3 py-6 text-center">
              No acquisition records match this filter.
            </p>
          ) : (
            <SortableList
              ids={visible.map((row) => state.acqIds[row.recordIndex])}
              disabled={!inPrlOrder}
              onReorder={(from, to) =>
                moveRow(visible[from].recordIndex, visible[to].recordIndex)
              }
            >
              <Virtuoso
                ref={listRef}
                style={{ height: 600 }}
                data={visible}
                computeItemKey={(_index, row) =>
                  state.acqIds[row.recordIndex]
                }
                initialTopMostItemIndex={
                  focusedIndex == null
                    ? 0
                    : Math.max(
                        0,
                        visible.findIndex(
                          (row) => row.recordIndex === focusedIndex,
                        ),
                      )
                }
                itemContent={(_itemIndex, row) => {
                const rowId = state.acqIds[row.recordIndex];
                const isOpen = openRows.includes(String(row.recordIndex));
                const referenceLinks = row.references.map(
                  ({ systemRecord, systemIndex }) => ({
                    systemIndex,
                    label: systemReferenceLabel(systemRecord, systemIndex),
                  }),
                );
                return (
                  <SortableRow id={rowId} disabled={!inPrlOrder}>
                    {(drag) => (
                      <div
                        id={`acq-${row.recordIndex}`}
                        className={`border-t border-border/30 text-xs ${
                          isOpen ? "bg-bg/30" : ""
                        } ${drag.isDragging ? "relative z-30 shadow-xl" : ""}`}
                      >
                        <AcqSummaryRow
                          index={row.recordIndex}
                          type={row.type}
                          details={row.details}
                          references={referenceLinks}
                          onNavigateSys={navigateToSystemRecord}
                          isOpen={isOpen}
                          onToggle={() => toggleExpanded(row.recordIndex)}
                          onMoveUp={() =>
                            moveRow(
                              row.recordIndex,
                              Math.max(0, row.recordIndex - 1),
                            )
                          }
                          onMoveDown={() =>
                            moveRow(
                              row.recordIndex,
                              Math.min(records.length - 1, row.recordIndex + 1),
                            )
                          }
                          onRemove={() => removeRow(row.recordIndex)}
                          canMoveUp={inPrlOrder && row.recordIndex > 0}
                          canMoveDown={
                            inPrlOrder && row.recordIndex < records.length - 1
                          }
                          reorderEnabled={inPrlOrder}
                          drag={drag}
                        />
                        {isOpen && (
                      <div className="px-3 pb-3 pt-2 border-t border-border/30 bg-bg/20">
                        <div className="mb-3 rounded border border-border/60 bg-bg/30 p-2">
                          <div className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-dimmed">
                            Used by systems
                          </div>
                          {referenceLinks.length === 0 ? (
                            <span className="text-[11px] text-dimmed">
                              No system records reference this acquisition.
                            </span>
                          ) : (
                            <div className="flex flex-wrap gap-1.5">
                              {referenceLinks.map((reference) => (
                                <a
                                  key={reference.systemIndex}
                                  href={`#sys-${reference.systemIndex}`}
                                  title={reference.label}
                                  className="max-w-full rounded border border-accent-blue/30 bg-accent-blue/10 px-2 py-1 font-mono text-[10px] text-accent-blue hover:border-accent-blue/60"
                                  onClick={(event) => {
                                    event.preventDefault();
                                    navigateToSystemRecord(
                                      reference.systemIndex,
                                    );
                                  }}
                                >
                                  {reference.label}
                                </a>
                              ))}
                            </div>
                          )}
                        </div>
                        <AcqRowEditor
                          mode={mode}
                          record={row.record}
                          onPatch={(mutator) =>
                            dispatch({
                              type: "patchAcq",
                              index: row.recordIndex,
                              mutator,
                            })
                          }
                          errors={errors}
                          errorPrefix={`acq[${row.recordIndex}]`}
                        />
                      </div>
                        )}
                      </div>
                    )}
                  </SortableRow>
                );
                }}
              />
            </SortableList>
          )}
        </div>
      )}
      </Card>
    </div>
  );
}

const COLS =
  "grid grid-cols-[24px_28px_44px_minmax(150px,0.9fr)_minmax(180px,1.4fr)_minmax(130px,0.8fr)_120px] items-center gap-2 px-3 py-1.5";

function AcqHeaderRow({
  sort,
  onSort,
}: {
  sort: { key: AcqSortKey; direction: SortDirection };
  onSort: (key: AcqSortKey) => void;
}) {
  return (
    <div
      className={`${COLS} bg-bg/60 border-b border-border/40 text-[10px] uppercase tracking-wider text-dimmed font-semibold`}
    >
      <span />
      <span />
      <SortableColumnHeader
        label="#"
        active={sort.key === "index"}
        direction={sort.direction}
        onClick={() => onSort("index")}
      />
      <SortableColumnHeader
        label="Type"
        active={sort.key === "type"}
        direction={sort.direction}
        onClick={() => onSort("type")}
      />
      <SortableColumnHeader
        label="Details"
        active={sort.key === "details"}
        direction={sort.direction}
        onClick={() => onSort("details")}
      />
      <SortableColumnHeader
        label="Used by"
        active={sort.key === "references"}
        direction={sort.direction}
        onClick={() => onSort("references")}
      />
      <span className="text-right">Actions</span>
    </div>
  );
}

function AcqSummaryRow({
  index,
  type,
  details,
  references,
  onNavigateSys,
  isOpen,
  onToggle,
  onMoveUp,
  onMoveDown,
  onRemove,
  canMoveUp,
  canMoveDown,
  reorderEnabled,
  drag,
}: {
  index: number;
  type: string;
  details: string;
  references: { systemIndex: number; label: string }[];
  onNavigateSys: (index: number) => void;
  isOpen: boolean;
  onToggle: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  onRemove: () => void;
  canMoveUp: boolean;
  canMoveDown: boolean;
  reorderEnabled: boolean;
  drag: SortableDragHandleProps;
}) {
  return (
    <div
      className={`${COLS} cursor-pointer hover:bg-bg/40`}
      onClick={onToggle}
    >
      <DragHandle
        listeners={drag.listeners}
        attributes={drag.attributes}
        setActivatorNodeRef={drag.setActivatorNodeRef}
        disabled={!reorderEnabled}
      />
      <button
        onClick={(event) => {
          event.stopPropagation();
          onToggle();
        }}
        className="text-muted hover:text-primary text-xs"
      >
        {isOpen ? "▾" : "▸"}
      </button>
      <span className="font-mono text-dimmed">{index}</span>
      <span className="font-mono text-primary truncate" title={type}>
        {type}
      </span>
      <span className="font-mono truncate" title={details}>
        {details}
      </span>
      <span
        className="font-mono truncate"
        title={references.map((reference) => reference.label).join(", ")}
      >
        {references.length === 0
          ? "—"
          : references.length > 1
            ? `${references.length} systems`
          : references.map((reference, index) => (
              <span key={reference.systemIndex}>
                {index > 0 && ", "}
                <a
                  href={`#sys-${reference.systemIndex}`}
                  className="text-accent-blue hover:underline"
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onNavigateSys(reference.systemIndex);
                  }}
                >
                  SYS #{reference.systemIndex}
                </a>
              </span>
            ))}
      </span>
      <div
        className="flex items-center justify-end gap-1"
        onClick={(event) => event.stopPropagation()}
      >
        <button
          onClick={onMoveUp}
          disabled={!canMoveUp}
          title={reorderEnabled ? "Move up" : "Use PRL order to reorder"}
          className="text-muted hover:text-primary disabled:opacity-30 text-[11px] px-1"
        >
          ▲
        </button>
        <button
          onClick={onMoveDown}
          disabled={!canMoveDown}
          title={reorderEnabled ? "Move down" : "Use PRL order to reorder"}
          className="text-muted hover:text-primary disabled:opacity-30 text-[11px] px-1"
        >
          ▼
        </button>
        <button
          onClick={onRemove}
          className="text-accent-red text-[11px] hover:underline ml-1"
        >
          Remove
        </button>
      </div>
    </div>
  );
}
