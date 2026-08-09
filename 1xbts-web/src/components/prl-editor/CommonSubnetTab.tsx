import { Dispatch, useMemo } from "react";
import { Card } from "@/components/card";
import { v4 as uuid } from "@/lib/uuid-lite";
import {
  EditorState,
  EditorAction,
  subnetRecordsOf,
  sysRecordsOf,
} from "./state";
import { ErrorMap } from "./validation";
import { emptyCommonSubnet } from "./builders";
import { NumericInput } from "./shared/NumericInput";
import { HexBytesInput } from "./shared/HexBytesInput";
import { systemReferenceLabel } from "./system-label";
import {
  compareSortValues,
  FilterField,
  FILTER_INPUT_CLASS,
  FILTER_SELECT_CLASS,
  SortableColumnHeader,
  SortDirection,
  TableFilters,
} from "./shared/TableFilters";
import { SearchableSelect } from "./shared/SearchableSelect";
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
  SortableList,
  SortableRow,
} from "./shared/SortableList";

type SubnetSortKey = "index" | "length" | "subnet" | "references";
const SUBNET_SORT_KEYS: readonly SubnetSortKey[] = [
  "index",
  "length",
  "subnet",
  "references",
];

export function CommonSubnetTab({
  state,
  dispatch,
  errors,
  onNavigateSys,
}: {
  state: EditorState;
  dispatch: Dispatch<EditorAction>;
  errors: ErrorMap;
  onNavigateSys: (index: number) => void;
}) {
  const records = subnetRecordsOf(state.draft);
  const systemRecords = sysRecordsOf(state.draft);
  const [indexFilter, setIndexFilter] = useUrlStringState("subnetRow");
  const [lengthFilter, setLengthFilter] = useUrlStringState(
    "subnetLength",
    "all",
  );
  const [subnetFilter, setSubnetFilter] = useUrlStringState("subnetHex");
  const [referenceFilter, setReferenceFilter] = useUrlStringState(
    "subnetReference",
    "all",
  );
  const [openRows, setOpenRows] = useUrlStringListState("subnetOpen");
  const [sort, setSort] = useUrlSortState(
    "subnetSort",
    SUBNET_SORT_KEYS,
    { key: "index", direction: "asc" },
  );

  const rows = useMemo(
    () =>
      records.map((record, index) => ({
        record,
        index,
        references: systemRecords
          .map((systemRecord, systemIndex) => ({ systemRecord, systemIndex }))
          .filter(
            ({ systemRecord }) =>
              "hrpd" in systemRecord &&
              systemRecord.hrpd?.subnetCommonIncluded &&
              systemRecord.hrpd.subnetCommonOffset === index,
          ),
      })),
    [records, systemRecords],
  );
  const lengthOptions = useMemo(
    () =>
      [...new Set(rows.map((row) => row.record.subnetCommonLengthOctets))].sort(
        (left, right) => left - right,
      ),
    [rows],
  );
  const filtered = useMemo(
    () =>
      rows.filter((row) => {
        if (indexFilter !== "" && row.index !== Number(indexFilter)) return false;
        if (
          lengthFilter !== "all" &&
          row.record.subnetCommonLengthOctets !== Number(lengthFilter)
        ) {
          return false;
        }
        if (
          subnetFilter !== "" &&
          row.record.subnetCommonHex.toUpperCase() !==
            subnetFilter.trim().toUpperCase()
        ) {
          return false;
        }
        if (referenceFilter === "used" && row.references.length === 0) return false;
        if (referenceFilter === "unused" && row.references.length > 0) return false;
        if (
          referenceFilter.startsWith("sys:") &&
          !row.references.some(
            ({ systemIndex }) =>
              systemIndex === Number(referenceFilter.slice(4)),
          )
        ) {
          return false;
        }
        return true;
      }),
    [indexFilter, lengthFilter, referenceFilter, rows, subnetFilter],
  );
  const visible = useMemo(() => {
    const next = [...filtered];
    next.sort((left, right) => {
      const leftValue =
        sort.key === "index"
          ? left.index
          : sort.key === "length"
            ? left.record.subnetCommonLengthOctets
            : sort.key === "subnet"
              ? left.record.subnetCommonHex
              : left.references.length;
      const rightValue =
        sort.key === "index"
          ? right.index
          : sort.key === "length"
            ? right.record.subnetCommonLengthOctets
            : sort.key === "subnet"
              ? right.record.subnetCommonHex
              : right.references.length;
      return compareSortValues(leftValue, rightValue, sort.direction);
    });
    return next;
  }, [filtered, sort]);

  const filtersActive =
    indexFilter !== "" ||
    lengthFilter !== "all" ||
    subnetFilter !== "" ||
    referenceFilter !== "all";
  const inPrlOrder = sort.key === "index" && sort.direction === "asc";

  const clearFilters = () => {
    setIndexFilter("");
    setLengthFilter("all");
    setSubnetFilter("");
    setReferenceFilter("all");
  };
  const changeSort = (key: SubnetSortKey) => {
    setSort((current) => ({
      key,
      direction:
        current.key === key && current.direction === "asc" ? "desc" : "asc",
    }));
  };
  const toggleExpanded = (index: number) => {
    setOpenRows((current) => toggleOpenRow(current, index));
  };
  const addRow = () => {
    const id = uuid();
    dispatch({ type: "addSubnet", record: emptyCommonSubnet(), id });
    setOpenRows((current) => toggleOpenRow(current, records.length));
    setSort({ key: "index", direction: "asc" });
    clearFilters();
    requestAnimationFrame(() => {
      document.getElementById(`subnet-${records.length}`)?.scrollIntoView({
        behavior: "smooth",
        block: "center",
      });
    });
  };

  const moveRow = (from: number, to: number) => {
    setOpenRows((current) => remapOpenRowsAfterMove(current, from, to));
    dispatch({ type: "reorderSubnet", from, to });
    setIndexFilter((current) =>
      current === String(from) ? String(to) : current,
    );
  };

  const removeRow = (index: number) => {
    setOpenRows((current) => remapOpenRowsAfterRemove(current, index));
    dispatch({ type: "removeSubnet", index });
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
        title="Filter subnets"
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
          ...(lengthFilter !== "all"
            ? [
                {
                  label: "Length",
                  value: `${lengthFilter} octets`,
                  onRemove: () => setLengthFilter("all"),
                },
              ]
            : []),
          ...(subnetFilter !== ""
            ? [
                {
                  label: "Subnet",
                  value: subnetFilter,
                  onRemove: () => setSubnetFilter(""),
                },
              ]
            : []),
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
            aria-label="Common subnet row index"
          />
        </FilterField>
        <FilterField label="Length" className="min-w-24">
          <select
            className={FILTER_SELECT_CLASS}
            value={lengthFilter}
            onChange={(event) => setLengthFilter(event.target.value)}
            aria-label="Common subnet length"
          >
            <option value="all">All lengths</option>
            {lengthOptions.map((length) => (
              <option key={length} value={length}>
                {length} octets
              </option>
            ))}
          </select>
        </FilterField>
        <FilterField label="Subnet hex" className="w-44">
          <input
            type="text"
            className={FILTER_INPUT_CLASS}
            value={subnetFilter}
            onChange={(event) => setSubnetFilter(event.target.value)}
            aria-label="Exact common subnet hex"
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

      <Card title={`Common Subnet Table (${records.length})`}>
      <p className="text-dimmed text-[11px] mb-3">
        Common subnet records are referenced by HRPD system records&apos;
        SUBNET_COMMON_OFFSET. Each record carries a number of octets of
        most-significant HRPD subnet bits.
      </p>
      <div className="flex justify-end mb-3">
        <button
          type="button"
          onClick={addRow}
          className="text-accent-blue text-xs hover:underline"
        >
          + Add row
        </button>
      </div>

      {records.length === 0 ? (
        <p className="text-dimmed text-xs">No common subnet records.</p>
      ) : (
        <div className="border border-border rounded overflow-hidden">
          <SubnetHeaderRow sort={sort} onSort={changeSort} />
          {visible.length === 0 ? (
            <p className="text-dimmed text-xs px-3 py-6 text-center">
              No common subnet records match these filters.
            </p>
          ) : (
            <SortableList
              ids={visible.map(({ index }) => state.subnetIds[index])}
              disabled={!inPrlOrder}
              onReorder={(from, to) =>
                moveRow(visible[from].index, visible[to].index)
              }
            >
              {visible.map(({ record, index, references }) => {
                const rowId = state.subnetIds[index];
                const isOpen = openRows.includes(String(index));
                return (
                  <SortableRow key={rowId} id={rowId} disabled={!inPrlOrder}>
                    {(drag) => (
                      <div
                        id={`subnet-${index}`}
                        className={`border-t border-border/30 text-xs ${
                          isOpen ? "bg-bg/30" : ""
                        } ${drag.isDragging ? "relative z-30 shadow-xl" : ""}`}
                      >
                        <div
                          className={`${COLS} cursor-pointer hover:bg-bg/40`}
                          onClick={() => toggleExpanded(index)}
                        >
                          <DragHandle
                            listeners={drag.listeners}
                            attributes={drag.attributes}
                            setActivatorNodeRef={drag.setActivatorNodeRef}
                            disabled={!inPrlOrder}
                          />
                          <button
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              toggleExpanded(index);
                            }}
                            className="text-muted hover:text-primary text-xs"
                          >
                            {isOpen ? "▾" : "▸"}
                          </button>
                    <span className="font-mono text-dimmed">{index}</span>
                    <span className="font-mono">
                      {record.subnetCommonLengthOctets} octets
                    </span>
                    <span
                      className="font-mono truncate"
                      title={record.subnetCommonHex}
                    >
                      {record.subnetCommonHex || "—"}
                    </span>
                    <span className="font-mono truncate">
                      {references.length === 0 ? (
                        "—"
                      ) : references.length > 1 ? (
                        `${references.length} systems`
                      ) : (
                        <a
                          href={`#sys-${references[0].systemIndex}`}
                          className="text-accent-blue hover:underline"
                          onClick={(event) => {
                            event.preventDefault();
                            event.stopPropagation();
                            navigateToSystemRecord(
                              references[0].systemIndex,
                            );
                          }}
                        >
                          SYS #{references[0].systemIndex}
                        </a>
                      )}
                    </span>
                    <div
                      className="flex items-center justify-end gap-1"
                      onClick={(event) => event.stopPropagation()}
                    >
                      <button
                        type="button"
                        onClick={() => moveRow(index, Math.max(0, index - 1))}
                        disabled={!inPrlOrder || index === 0}
                        title={
                          inPrlOrder ? "Move up" : "Use PRL order to reorder"
                        }
                        className="text-muted hover:text-primary disabled:opacity-30 text-[11px] px-1"
                      >
                        ▲
                      </button>
                      <button
                        type="button"
                        onClick={() =>
                          moveRow(
                            index,
                            Math.min(records.length - 1, index + 1),
                          )
                        }
                        disabled={!inPrlOrder || index === records.length - 1}
                        title={
                          inPrlOrder ? "Move down" : "Use PRL order to reorder"
                        }
                        className="text-muted hover:text-primary disabled:opacity-30 text-[11px] px-1"
                      >
                        ▼
                      </button>
                      <button
                        type="button"
                        onClick={() => removeRow(index)}
                        className="text-accent-red text-[11px] hover:underline ml-1"
                      >
                        Remove
                      </button>
                    </div>
                        </div>
                        {isOpen && (
                    <div className="grid grid-cols-2 gap-2 px-3 pb-3 pt-2 border-t border-border/30 bg-bg/20">
                      <div className="col-span-full rounded border border-border/60 bg-bg/30 p-2">
                        <div className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-dimmed">
                          Used by systems
                        </div>
                        {references.length === 0 ? (
                          <span className="text-[11px] text-dimmed">
                            No system records reference this subnet.
                          </span>
                        ) : (
                          <div className="flex flex-wrap gap-1.5">
                            {references.map(
                              ({ systemRecord, systemIndex }) => (
                                <a
                                  key={systemIndex}
                                  href={`#sys-${systemIndex}`}
                                  title={systemReferenceLabel(
                                    systemRecord,
                                    systemIndex,
                                  )}
                                  className="max-w-full rounded border border-accent-blue/30 bg-accent-blue/10 px-2 py-1 font-mono text-[10px] text-accent-blue hover:border-accent-blue/60"
                                  onClick={(event) => {
                                    event.preventDefault();
                                    navigateToSystemRecord(systemIndex);
                                  }}
                                >
                                  {systemReferenceLabel(
                                    systemRecord,
                                    systemIndex,
                                  )}
                                </a>
                              ),
                            )}
                          </div>
                        )}
                      </div>
                      <NumericInput
                        label="SUBNET_COMMON_LENGTH (octets)"
                        min={0}
                        max={15}
                        value={record.subnetCommonLengthOctets}
                        onChange={(value) =>
                          dispatch({
                            type: "patchSubnet",
                            index,
                            mutator: (draft) => {
                              draft.subnetCommonLengthOctets = value;
                              const expectedCharacters = value * 2;
                              if (
                                draft.subnetCommonHex.length < expectedCharacters
                              ) {
                                draft.subnetCommonHex += "0".repeat(
                                  expectedCharacters -
                                    draft.subnetCommonHex.length,
                                );
                              } else if (
                                draft.subnetCommonHex.length > expectedCharacters
                              ) {
                                draft.subnetCommonHex =
                                  draft.subnetCommonHex.slice(
                                    0,
                                    expectedCharacters,
                                  );
                              }
                            },
                          })
                        }
                        error={errors.get(
                          `subnet[${index}].subnetCommonLengthOctets`,
                        )}
                      />
                      <HexBytesInput
                        label="SUBNET_COMMON (hex)"
                        lengthBits={record.subnetCommonLengthOctets * 8}
                        value={record.subnetCommonHex}
                        onChange={(value) =>
                          dispatch({
                            type: "patchSubnet",
                            index,
                            mutator: (draft) => {
                              draft.subnetCommonHex = value;
                            },
                          })
                        }
                        error={errors.get(`subnet[${index}].subnetCommonHex`)}
                      />
                    </div>
                        )}
                      </div>
                    )}
                  </SortableRow>
                );
              })}
            </SortableList>
          )}
        </div>
      )}
      </Card>
    </div>
  );
}

const COLS =
  "grid grid-cols-[24px_28px_44px_100px_minmax(180px,1fr)_minmax(130px,0.7fr)_120px] items-center gap-2 px-3 py-1.5";

function SubnetHeaderRow({
  sort,
  onSort,
}: {
  sort: { key: SubnetSortKey; direction: SortDirection };
  onSort: (key: SubnetSortKey) => void;
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
        label="Length"
        active={sort.key === "length"}
        direction={sort.direction}
        onClick={() => onSort("length")}
      />
      <SortableColumnHeader
        label="Subnet"
        active={sort.key === "subnet"}
        direction={sort.direction}
        onClick={() => onSort("subnet")}
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
