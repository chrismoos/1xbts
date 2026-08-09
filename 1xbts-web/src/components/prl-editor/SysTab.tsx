import { Dispatch, useMemo, useRef } from "react";
import { Card } from "@/components/card";
import { Virtuoso, VirtuosoHandle } from "react-virtuoso";
import { v4 as uuid } from "@/lib/uuid-lite";
import {
  PrlExtSysRecord,
  PrlExtSysRecordType,
  PrlPrefNeg,
  PrlSysRecord,
} from "@/lib/proto/hlr/v1/service";
import {
  EditorState,
  EditorAction,
  modeOf,
  sysRecordsOf,
  acqRecordsOf,
  subnetRecordsOf,
} from "./state";
import { ErrorMap } from "./validation";
import { emptyClassicSys, emptyExtSys } from "./builders";
import { EXT_SYS_TYPE_OPTIONS } from "@/lib/prl-options";
import { SysRowEditor } from "./sys/SysRowEditor";
import { roamIndLabel } from "./shared/RoamingIndicatorSelect";
import { acqDetailSummary, acqShortLabel } from "./acq-label";
import {
  compareSortValues,
  FilterField,
  FILTER_INPUT_CLASS,
  SortableColumnHeader,
  SortDirection,
  TableFilters,
} from "./shared/TableFilters";
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

type SysSortKey = "index" | "type" | "identity" | "acq" | "pref" | "roam";
const SYS_SORT_KEYS: readonly SysSortKey[] = [
  "index",
  "type",
  "identity",
  "acq",
  "pref",
  "roam",
];

export function SysTab({
  state,
  dispatch,
  errors,
  onNavigateAcq,
  focusedIndex,
}: {
  state: EditorState;
  dispatch: Dispatch<EditorAction>;
  errors: ErrorMap;
  onNavigateAcq: (index: number) => void;
  focusedIndex?: number;
}) {
  const mode = modeOf(state.draft);
  const records = sysRecordsOf(state.draft);
  const acqRecords = acqRecordsOf(state.draft);
  const subnetCount = subnetRecordsOf(state.draft).length;
  const [indexFilter, setIndexFilter] = useUrlStringState(
    "sysRow",
    focusedIndex == null ? "" : String(focusedIndex),
  );
  const [typeFilters, setTypeFilters] = useUrlStringListState("sysType");
  const [acqFilters, setAcqFilters] = useUrlStringListState("sysAcq");
  const [prefFilters, setPrefFilters] = useUrlStringListState("sysPreference");
  const [roamFilters, setRoamFilters] = useUrlStringListState("sysRoaming");
  const [sidFilter, setSidFilter] = useUrlStringState("sysSid");
  const [nidFilter, setNidFilter] = useUrlStringState("sysNid");
  const [mccFilter, setMccFilter] = useUrlStringState("sysMcc");
  const [mncFilter, setMncFilter] = useUrlStringState("sysMnc");
  const [subnetFilter, setSubnetFilter] = useUrlStringState("sysSubnet");
  const [openRows, setOpenRows] = useUrlStringListState("sysOpen");
  const [sort, setSort] = useUrlSortState("sysSort", SYS_SORT_KEYS, {
    key: "index",
    direction: "asc",
  });
  const listRef = useRef<VirtuosoHandle>(null);

  const rows = useMemo(
    () =>
      records.map((record, index) => ({
        r: record,
        recordIndex: index,
        cells: formatCells(mode, record),
      })),
    [mode, records],
  );
  const typeOptions = useMemo(
    () => [...new Set(rows.map((row) => row.cells.type))].sort(),
    [rows],
  );
  const acqOptions = useMemo(
    () => [...new Set(rows.map((row) => row.r.acqIndex))].sort((a, b) => a - b),
    [rows],
  );
  const roamOptions = useMemo(
    () =>
      [
        ...new Set(
          rows.flatMap((row) =>
            row.r.roamingIndicator ? [row.r.roamingIndicator.raw] : [],
          ),
        ),
      ].sort((a, b) => a - b),
    [rows],
  );

  const filtered = useMemo(
    () =>
      rows.filter((row) => {
        if (
          indexFilter !== "" &&
          row.recordIndex !== Number(indexFilter)
        ) {
          return false;
        }
        if (
          typeFilters.length > 0 &&
          !typeFilters.includes(row.cells.type)
        ) {
          return false;
        }
        if (
          acqFilters.length > 0 &&
          !acqFilters.includes(String(row.r.acqIndex))
        ) {
          return false;
        }
        if (
          prefFilters.length > 0 &&
          !prefFilters.includes(row.cells.pref)
        ) {
          return false;
        }
        const roamingRaw = row.r.roamingIndicator?.raw;
        if (
          roamFilters.length > 0 &&
          !roamFilters.includes(
            roamingRaw == null ? "none" : String(roamingRaw),
          )
        ) {
          return false;
        }
        if (sidFilter !== "" && systemSid(row.r) !== Number(sidFilter)) {
          return false;
        }
        if (nidFilter !== "" && systemNid(row.r) !== Number(nidFilter)) {
          return false;
        }
        const mccMnc = systemMccMnc(row.r);
        if (mccFilter !== "" && mccMnc?.mcc !== mccFilter.trim()) return false;
        if (mncFilter !== "" && mccMnc?.mnc !== mncFilter.trim()) return false;
        if (
          subnetFilter !== "" &&
          systemSubnet(row.r) !== subnetFilter.trim().toUpperCase()
        ) {
          return false;
        }
        return true;
      }),
    [
      acqFilters,
      indexFilter,
      mccFilter,
      mncFilter,
      nidFilter,
      prefFilters,
      roamFilters,
      rows,
      sidFilter,
      subnetFilter,
      typeFilters,
    ],
  );

  const filtersActive =
    indexFilter !== "" ||
    typeFilters.length > 0 ||
    acqFilters.length > 0 ||
    prefFilters.length > 0 ||
    roamFilters.length > 0 ||
    sidFilter !== "" ||
    nidFilter !== "" ||
    mccFilter !== "" ||
    mncFilter !== "" ||
    subnetFilter !== "";

  const visible = useMemo(() => {
    const next = [...filtered];
    next.sort((left, right) => {
      const leftValue =
        sort.key === "index"
          ? left.recordIndex
          : sort.key === "type"
            ? left.cells.type
            : sort.key === "identity"
              ? left.cells.identity
              : sort.key === "acq"
                ? left.r.acqIndex
                : sort.key === "pref"
                  ? left.cells.pref
                  : left.cells.roam;
      const rightValue =
        sort.key === "index"
          ? right.recordIndex
          : sort.key === "type"
            ? right.cells.type
            : sort.key === "identity"
              ? right.cells.identity
              : sort.key === "acq"
                ? right.r.acqIndex
                : sort.key === "pref"
                  ? right.cells.pref
                  : right.cells.roam;
      return compareSortValues(leftValue, rightValue, sort.direction);
    });
    return next;
  }, [filtered, sort]);

  const changeSort = (key: SysSortKey) => {
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
    setAcqFilters([]);
    setPrefFilters([]);
    setRoamFilters([]);
    setSidFilter("");
    setNidFilter("");
    setMccFilter("");
    setMncFilter("");
    setSubnetFilter("");
  };

  const changeTypeFilters = (next: string[]) => {
    setTypeFilters(next);
    if (next.length > 0 && !next.includes("cdma2000")) {
      setSidFilter("");
      setNidFilter("");
    }
    if (next.length > 0 && !next.includes("HRPD")) setSubnetFilter("");
    if (
      next.length > 0 &&
      !next.some((type) => type.startsWith("MCC-MNC"))
    ) {
      setMccFilter("");
      setMncFilter("");
    }
  };

  const addClassic = () => {
    addSystemRecord(emptyClassicSys());
    clearFilters();
  };

  const addExt = (sysRecordType: PrlExtSysRecordType) => {
    addSystemRecord(emptyExtSys(sysRecordType));
    clearFilters();
  };

  const addSystemRecord = (record: PrlSysRecord | PrlExtSysRecord) => {
    const id = uuid();
    dispatch({ type: "addSys", record, id });
    setOpenRows((current) => toggleOpenRow(current, records.length));
    setSort({ key: "index", direction: "asc" });
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
    dispatch({ type: "reorderSys", from, to });
    setIndexFilter((current) =>
      current === String(from) ? String(to) : current,
    );
  };

  const removeRow = (index: number) => {
    setOpenRows((current) => remapOpenRowsAfterRemove(current, index));
    dispatch({ type: "removeSys", index });
    setIndexFilter((current) =>
      current === String(index) ? "" : current,
    );
  };

  const navigateToAcquisitionRecord = (index: number) => {
    onNavigateAcq(index);
  };

  const acqContext = {
    acqRecords,
    patchAcq: (
      index: number,
      mutator: (draft: (typeof acqRecords)[number]) => void,
    ) => dispatch({ type: "patchAcq", index, mutator }),
  };

  return (
    <div className="grid items-start gap-3 xl:grid-cols-[15rem_minmax(0,1fr)]">
      <TableFilters
        title="Filter systems"
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
            value,
            onRemove: () =>
              changeTypeFilters(
                typeFilters.filter((selected) => selected !== value),
              ),
          })),
          ...acqFilters.map((value) => ({
            label: "Acquisition",
            value: `ACQ #${value}`,
            onRemove: () =>
              setAcqFilters((current) =>
                current.filter((selected) => selected !== value),
              ),
          })),
          ...prefFilters.map((value) => ({
            label: "Preference",
            value: value === "PREF" ? "Preferred" : "Negative",
            onRemove: () =>
              setPrefFilters((current) =>
                current.filter((selected) => selected !== value),
              ),
          })),
          ...roamFilters.map((value) => ({
            label: "Roaming",
            value:
              value === "none" ? "No indication" : roamIndLabel(Number(value)),
            onRemove: () =>
              setRoamFilters((current) =>
                current.filter((selected) => selected !== value),
              ),
          })),
          ...(sidFilter !== ""
            ? [
                {
                  label: "SID",
                  value: sidFilter,
                  onRemove: () => setSidFilter(""),
                },
              ]
            : []),
          ...(nidFilter !== ""
            ? [
                {
                  label: "NID",
                  value: nidFilter,
                  onRemove: () => setNidFilter(""),
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
          ...(mccFilter !== ""
            ? [
                {
                  label: "MCC",
                  value: mccFilter,
                  onRemove: () => setMccFilter(""),
                },
              ]
            : []),
          ...(mncFilter !== ""
            ? [
                {
                  label: "MNC",
                  value: mncFilter,
                  onRemove: () => setMncFilter(""),
                },
              ]
            : []),
        ]}
        quickFilters={[
          {
            label: "cdma2000",
            active: typeFilters.includes("cdma2000"),
            onClick: () =>
              changeTypeFilters(
                typeFilters.includes("cdma2000")
                  ? typeFilters.filter((type) => type !== "cdma2000")
                  : [...typeFilters, "cdma2000"],
              ),
          },
          ...(mode === "extended"
            ? [
                {
                  label: "HRPD",
                  active: typeFilters.includes("HRPD"),
                  onClick: () =>
                    changeTypeFilters(
                      typeFilters.includes("HRPD")
                        ? typeFilters.filter((type) => type !== "HRPD")
                        : [...typeFilters, "HRPD"],
                    ),
                },
              ]
            : []),
          {
            label: "Preferred",
            active: prefFilters.includes("PREF"),
            onClick: () =>
              setPrefFilters((current) =>
                current.includes("PREF")
                  ? current.filter((value) => value !== "PREF")
                  : [...current, "PREF"],
              ),
          },
          {
            label: "Negative",
            active: prefFilters.includes("NEG"),
            onClick: () =>
              setPrefFilters((current) =>
                current.includes("NEG")
                  ? current.filter((value) => value !== "NEG")
                  : [...current, "NEG"],
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
            aria-label="System row index"
          />
        </FilterField>
        <FilterField label="System type" className="min-w-36">
          <SearchableMultiSelect
            values={typeFilters}
            options={typeOptions.map((type) => ({ value: type, label: type }))}
            onChange={changeTypeFilters}
            placeholder="All system types"
            ariaLabel="System types"
          />
        </FilterField>
        <FilterField label="Acquisition" className="min-w-64">
          <SearchableMultiSelect
            values={acqFilters}
            options={acqOptions.map((index) => ({
              value: String(index),
              label: `ACQ #${index}${
                acqRecords[index]
                  ? ` · ${acqShortLabel(acqRecords[index], mode)}`
                  : " — Missing"
              }`,
              searchText: acqRecords[index]
                ? `${acqDetailSummary(acqRecords[index])} ${JSON.stringify(acqRecords[index])}`
                : "missing",
            }))}
            onChange={setAcqFilters}
            placeholder="All acquisitions"
            ariaLabel="Acquisition records"
          />
        </FilterField>
        <FilterField label="Preference" className="min-w-28">
          <SearchableMultiSelect
            values={prefFilters}
            options={[
              { value: "PREF", label: "Preferred" },
              { value: "NEG", label: "Negative" },
            ]}
            onChange={setPrefFilters}
            placeholder="Any preference"
            ariaLabel="Preferences"
          />
        </FilterField>
        <FilterField label="Roaming" className="min-w-44">
          <SearchableMultiSelect
            values={roamFilters}
            options={[
              { value: "none", label: "No indication" },
              ...roamOptions.map((raw) => ({
                value: String(raw),
                label: roamIndLabel(raw),
              })),
            ]}
            onChange={setRoamFilters}
            placeholder="Any indication"
            ariaLabel="Roaming indications"
          />
        </FilterField>
        {(mode === "classic" ||
          typeFilters.length === 0 ||
          typeFilters.includes("cdma2000")) && (
          <>
            <FilterField label="SID" className="w-24">
              <input
                type="number"
                min={0}
                className={FILTER_INPUT_CLASS}
                value={sidFilter}
                onChange={(event) => setSidFilter(event.target.value)}
                aria-label="Exact SID"
              />
            </FilterField>
            <FilterField label="NID" className="w-24">
              <input
                type="number"
                min={0}
                className={FILTER_INPUT_CLASS}
                value={nidFilter}
                onChange={(event) => setNidFilter(event.target.value)}
                aria-label="Exact NID"
              />
            </FilterField>
          </>
        )}
        {mode === "extended" &&
          (typeFilters.length === 0 || typeFilters.includes("HRPD")) && (
            <FilterField label="Subnet hex" className="w-36">
              <input
                type="text"
                className={FILTER_INPUT_CLASS}
                value={subnetFilter}
                onChange={(event) => setSubnetFilter(event.target.value)}
                aria-label="Exact HRPD subnet hex"
              />
            </FilterField>
          )}
        {mode === "extended" &&
          (typeFilters.length === 0 ||
            typeFilters.some((type) => type.startsWith("MCC-MNC"))) && (
            <>
              <FilterField label="MCC" className="w-20">
                <input
                  type="text"
                  inputMode="numeric"
                  className={FILTER_INPUT_CLASS}
                  value={mccFilter}
                  onChange={(event) => setMccFilter(event.target.value)}
                  aria-label="Exact MCC"
                />
              </FilterField>
              <FilterField label="MNC" className="w-20">
                <input
                  type="text"
                  inputMode="numeric"
                  className={FILTER_INPUT_CLASS}
                  value={mncFilter}
                  onChange={(event) => setMncFilter(event.target.value)}
                  aria-label="Exact MNC"
                />
              </FilterField>
            </>
          )}
      </TableFilters>

      <Card title={`System Records (${records.length})`}>
      <div className="flex items-center justify-end gap-2 mb-3 text-xs flex-wrap">
        {mode === "classic" ? (
          <button
            onClick={addClassic}
            className="text-accent-blue text-xs hover:underline"
          >
            + Add row
          </button>
        ) : (
          <select
            className="bg-bg border border-border rounded px-2 py-1"
            defaultValue=""
            onChange={(e) => {
              if (e.target.value) {
                addExt(Number(e.target.value) as PrlExtSysRecordType);
                e.target.value = "";
              }
            }}
          >
            <option value="">+ Add row…</option>
            {EXT_SYS_TYPE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        )}
      </div>

      {records.length === 0 ? (
        <p className="text-dimmed text-xs">
          No system records yet. Add one — these reference acquisition records
          by index.
        </p>
      ) : (
        <div className="border border-border rounded overflow-hidden">
          <SysHeaderRow mode={mode} sort={sort} onSort={changeSort} />
          {filtered.length === 0 ? (
            <p className="text-dimmed text-xs px-3 py-6 text-center">
              No system records match these filters.
            </p>
          ) : (
            <SortableList
              ids={visible.map((row) => state.sysIds[row.recordIndex])}
              disabled={!inPrlOrder}
              onReorder={(from, to) =>
                moveRow(visible[from].recordIndex, visible[to].recordIndex)
              }
            >
              <Virtuoso
                ref={listRef}
                style={{ height: 600 }}
                data={visible}
                computeItemKey={(_itemIndex, row) =>
                  state.sysIds[row.recordIndex]
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
                itemContent={(_i, { r, recordIndex }) => {
              const index = recordIndex;
              const rowId = state.sysIds[index];
              const isOpen = openRows.includes(String(index));
              const linkedAcq = acqRecords[r.acqIndex];
              return (
                <SortableRow id={rowId} disabled={!inPrlOrder}>
                  {(drag) => (
                    <div
                      id={`sys-${index}`}
                      className={`border-t border-border/30 text-xs ${
                        isOpen ? "bg-bg/30" : ""
                      } ${drag.isDragging ? "relative z-30 shadow-xl" : ""}`}
                    >
                      <SysSummaryRow
                        mode={mode}
                        record={r}
                        index={index}
                        isOpen={isOpen}
                        onToggle={() => toggleExpanded(index)}
                        onMoveUp={() => moveRow(index, Math.max(0, index - 1))}
                        onMoveDown={() =>
                          moveRow(index, Math.min(records.length - 1, index + 1))
                        }
                        onRemove={() => removeRow(index)}
                        canMoveUp={inPrlOrder && index > 0}
                        canMoveDown={inPrlOrder && index < records.length - 1}
                        reorderEnabled={inPrlOrder}
                        onNavigateAcq={() =>
                          navigateToAcquisitionRecord(r.acqIndex)
                        }
                        drag={drag}
                      />
                      {isOpen && (
                    <div className="px-3 pb-3 pt-2 border-t border-border/30 bg-bg/20">
                      <div className="mb-3 rounded border border-border/60 bg-bg/30 p-2">
                        <div className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-dimmed">
                          Uses acquisition
                        </div>
                        <a
                          href={`#acq-${r.acqIndex}`}
                          className="inline-flex max-w-full items-center gap-1.5 rounded border border-accent-blue/30 bg-accent-blue/10 px-2 py-1 font-mono text-[10px] text-accent-blue hover:border-accent-blue/60"
                          onClick={(event) => {
                            event.preventDefault();
                            navigateToAcquisitionRecord(r.acqIndex);
                          }}
                        >
                          <span>ACQ #{r.acqIndex}</span>
                          <span className="truncate text-muted">
                            {linkedAcq
                              ? `· ${acqShortLabel(linkedAcq, mode)} · ${acqDetailSummary(linkedAcq)}`
                              : "· Missing acquisition"}
                          </span>
                        </a>
                      </div>
                      <SysRowEditor
                        mode={mode}
                        record={r}
                        acq={acqContext}
                        subnetCount={subnetCount}
                        onPatch={(mutator) =>
                          dispatch({ type: "patchSys", index, mutator })
                        }
                        errors={errors}
                        errorPrefix={`sys[${index}]`}
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

// Column widths shared by header + row.
const COLS =
  "grid grid-cols-[24px_28px_44px_88px_minmax(120px,1fr)_60px_56px_minmax(140px,1fr)_120px] items-center gap-2 px-3 py-1.5";

function SysHeaderRow({
  mode,
  sort,
  onSort,
}: {
  mode: "classic" | "extended";
  sort: { key: SysSortKey; direction: SortDirection };
  onSort: (key: SysSortKey) => void;
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
        label={mode === "classic" ? "SID / NID" : "Identity"}
        active={sort.key === "identity"}
        direction={sort.direction}
        onClick={() => onSort("identity")}
      />
      <SortableColumnHeader
        label="Acq"
        active={sort.key === "acq"}
        direction={sort.direction}
        onClick={() => onSort("acq")}
      />
      <SortableColumnHeader
        label="Pref"
        active={sort.key === "pref"}
        direction={sort.direction}
        onClick={() => onSort("pref")}
      />
      <SortableColumnHeader
        label="Roam"
        active={sort.key === "roam"}
        direction={sort.direction}
        onClick={() => onSort("roam")}
      />
      <span className="text-right">Actions</span>
    </div>
  );
}

function SysSummaryRow({
  mode,
  record,
  index,
  isOpen,
  onToggle,
  onMoveUp,
  onMoveDown,
  onRemove,
  canMoveUp,
  canMoveDown,
  reorderEnabled,
  onNavigateAcq,
  drag,
}: {
  mode: "classic" | "extended";
  record: PrlSysRecord | PrlExtSysRecord;
  index: number;
  isOpen: boolean;
  onToggle: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  onRemove: () => void;
  canMoveUp: boolean;
  canMoveDown: boolean;
  reorderEnabled: boolean;
  onNavigateAcq: () => void;
  drag: SortableDragHandleProps;
}) {
  const cells = formatCells(mode, record);
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
        onClick={(e) => {
          e.stopPropagation();
          onToggle();
        }}
        className="text-muted hover:text-primary text-xs"
      >
        {isOpen ? "▾" : "▸"}
      </button>
      <span className="font-mono text-dimmed">{index}</span>
      <span className="font-mono text-primary">{cells.type}</span>
      <span className="font-mono truncate" title={cells.identity}>
        {cells.identity}
      </span>
      <a
        href={`#acq-${record.acqIndex}`}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          onNavigateAcq();
        }}
        className="font-mono text-accent-blue hover:underline"
        title={`Open ACQ #${record.acqIndex}`}
      >
        {cells.acq}
      </a>
      <span className="font-mono">{cells.pref}</span>
      <span className="font-mono truncate" title={cells.roam}>
        {cells.roam}
      </span>
      <div
        className="flex items-center justify-end gap-1"
        onClick={(e) => e.stopPropagation()}
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

function formatCells(
  mode: "classic" | "extended",
  record: PrlSysRecord | PrlExtSysRecord,
): {
  type: string;
  identity: string;
  acq: string;
  pref: string;
  roam: string;
} {
  const acq = `#${record.acqIndex}`;
  const pref =
    record.prefNeg === PrlPrefNeg.PRL_PREF_NEG_PREFERRED ? "PREF" : "NEG";
  const roam = record.roamingIndicator
    ? roamIndLabel(record.roamingIndicator.raw)
    : "—";
  if (mode === "classic") {
    const r = record as PrlSysRecord;
    const sid = r.sid === 0 ? "any" : String(r.sid);
    const nid = r.nid != null ? (r.nid === 0xffff ? "any" : String(r.nid)) : "—";
    return {
      type: "cdma2000",
      identity: `SID ${sid} / NID ${nid}`,
      acq,
      pref,
      roam,
    };
  }
  const r = record as PrlExtSysRecord;
  if (r.cdma2000) {
    const sid = r.cdma2000.sid === 0 ? "any" : String(r.cdma2000.sid);
    const nid =
      r.cdma2000.nid != null
        ? r.cdma2000.nid === 0xffff
          ? "any"
          : String(r.cdma2000.nid)
        : "—";
    return {
      type: "cdma2000",
      identity: `SID ${sid} / NID ${nid}`,
      acq,
      pref,
      roam,
    };
  }
  if (r.hrpd) {
    const lsb = r.hrpd.subnetLsbHex ? r.hrpd.subnetLsbHex.toUpperCase() : "—";
    const off =
      r.hrpd.subnetCommonIncluded && r.hrpd.subnetCommonOffset != null
        ? ` + Common #${r.hrpd.subnetCommonOffset}`
        : "";
    return {
      type: "HRPD",
      identity: `subnet ${lsb}/${r.hrpd.subnetLsbLengthBits}${off}`,
      acq,
      pref,
      roam,
    };
  }
  if (r.mccMnc) {
    const mcc =
      r.mccMnc.subtype000?.mcc ??
      r.mccMnc.subtype001?.mcc ??
      r.mccMnc.subtype010?.mcc ??
      r.mccMnc.subtype011?.mcc ??
      "?";
    const mnc =
      r.mccMnc.subtype000?.mnc ??
      r.mccMnc.subtype001?.mnc ??
      r.mccMnc.subtype010?.mnc ??
      r.mccMnc.subtype011?.mnc ??
      "?";
    const sub = r.mccMnc.subtype000
      ? "000"
      : r.mccMnc.subtype001
        ? "001"
        : r.mccMnc.subtype010
          ? "010"
          : r.mccMnc.subtype011
            ? "011"
            : "?";
    return {
      type: `MCC-MNC.${sub}`,
      identity: `MCC ${mcc} / MNC ${mnc}`,
      acq,
      pref,
      roam,
    };
  }
  if (r.raw) {
    return {
      type: `raw 0x${r.raw.sysRecordType.toString(16)}`,
      identity: "—",
      acq,
      pref,
      roam,
    };
  }
  return { type: "?", identity: "—", acq, pref, roam };
}

function systemSid(record: PrlSysRecord | PrlExtSysRecord): number | undefined {
  return "sid" in record ? record.sid : record.cdma2000?.sid;
}

function systemNid(record: PrlSysRecord | PrlExtSysRecord): number | undefined {
  return "sid" in record ? record.nid : record.cdma2000?.nid;
}

function systemMccMnc(
  record: PrlSysRecord | PrlExtSysRecord,
): { mcc: string; mnc: string } | undefined {
  if (!("mccMnc" in record) || !record.mccMnc) return undefined;
  return (
    record.mccMnc.subtype000 ??
    record.mccMnc.subtype001 ??
    record.mccMnc.subtype010 ??
    record.mccMnc.subtype011
  );
}

function systemSubnet(
  record: PrlSysRecord | PrlExtSysRecord,
): string | undefined {
  return "hrpd" in record ? record.hrpd?.subnetLsbHex.toUpperCase() : undefined;
}
