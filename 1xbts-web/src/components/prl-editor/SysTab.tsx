import { Dispatch, useEffect, useMemo, useRef, useState } from "react";
import { Card } from "@/components/card";
import { Virtuoso } from "react-virtuoso";
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

export function SysTab({
  state,
  dispatch,
  errors,
}: {
  state: EditorState;
  dispatch: Dispatch<EditorAction>;
  errors: ErrorMap;
}) {
  const mode = modeOf(state.draft);
  const records = sysRecordsOf(state.draft);
  const acqRecords = acqRecordsOf(state.draft);
  const subnetCount = subnetRecordsOf(state.draft).length;
  const [filter, setFilter] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const prevSysCount = useRef(state.sysIds.length);

  // Auto-expand newly added rows so the operator can edit them right away.
  useEffect(() => {
    if (state.sysIds.length > prevSysCount.current) {
      const newId = state.sysIds[state.sysIds.length - 1];
      if (newId) {
        setExpanded((s) => {
          const next = new Set(s);
          next.add(newId);
          return next;
        });
      }
    }
    prevSysCount.current = state.sysIds.length;
  }, [state.sysIds]);

  const filtered = useMemo(() => {
    if (!filter.trim()) return records.map((r, index) => ({ r, index }));
    const q = filter.trim().toLowerCase();
    return records
      .map((r, index) => ({ r, index }))
      .filter(({ r, index }) => {
        const sid =
          "sid" in r
            ? (r as PrlSysRecord).sid
            : ((r as PrlExtSysRecord).cdma2000?.sid ?? -1);
        const acq =
          "acqIndex" in r ? (r as PrlSysRecord | PrlExtSysRecord).acqIndex : 0;
        return (
          String(index).includes(q) ||
          String(sid).includes(q) ||
          `acq${acq}`.includes(q)
        );
      });
  }, [records, filter]);

  const addClassic = () =>
    dispatch({ type: "addSys", record: emptyClassicSys() });

  const addExt = (sysRecordType: PrlExtSysRecordType) =>
    dispatch({ type: "addSys", record: emptyExtSys(sysRecordType) });

  const toggleExpanded = (id: string) => {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setExpanded(next);
  };

  const acqContext = {
    acqRecords,
    patchAcq: (
      index: number,
      mutator: (draft: (typeof acqRecords)[number]) => void,
    ) => dispatch({ type: "patchAcq", index, mutator }),
  };

  return (
    <Card title={`SYS_TABLE (${records.length})`}>
      <div className="flex items-center gap-2 mb-3 text-xs flex-wrap">
        <input
          type="text"
          className="flex-1 bg-bg border border-border rounded px-2 py-1 min-w-40"
          placeholder="Filter by index, SID, or acq#…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
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
          No system records yet. Add one — these reference ACQ_TABLE rows by
          index.
        </p>
      ) : (
        <div className="border border-border rounded overflow-hidden">
          <SysHeaderRow mode={mode} />
          <Virtuoso
            style={{ height: 600 }}
            data={filtered}
            itemContent={(_i, { r, index }) => {
              const rowId = state.sysIds[index];
              const isOpen = expanded.has(rowId);
              return (
                <div
                  key={rowId}
                  className={`border-t border-border/30 text-xs ${
                    isOpen ? "bg-bg/30" : ""
                  }`}
                >
                  <SysSummaryRow
                    mode={mode}
                    record={r}
                    index={index}
                    isOpen={isOpen}
                    onToggle={() => toggleExpanded(rowId)}
                    onMoveUp={() =>
                      dispatch({
                        type: "reorderSys",
                        from: index,
                        to: Math.max(0, index - 1),
                      })
                    }
                    onMoveDown={() =>
                      dispatch({
                        type: "reorderSys",
                        from: index,
                        to: Math.min(records.length - 1, index + 1),
                      })
                    }
                    onRemove={() => dispatch({ type: "removeSys", index })}
                    canMoveUp={index > 0}
                    canMoveDown={index < records.length - 1}
                  />
                  {isOpen && (
                    <div className="px-3 pb-3 pt-2 border-t border-border/30 bg-bg/20">
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
              );
            }}
          />
        </div>
      )}
    </Card>
  );
}

// Column widths shared by header + row.
const COLS =
  "grid grid-cols-[28px_44px_88px_minmax(120px,1fr)_60px_56px_minmax(140px,1fr)_120px] items-center gap-2 px-3 py-1.5";

function SysHeaderRow({ mode }: { mode: "classic" | "extended" }) {
  return (
    <div
      className={`${COLS} bg-bg/60 border-b border-border/40 text-[10px] uppercase tracking-wider text-dimmed font-semibold`}
    >
      <span />
      <span>#</span>
      <span>Type</span>
      <span>{mode === "classic" ? "SID / NID" : "Identity"}</span>
      <span>Acq</span>
      <span>Pref</span>
      <span>Roam</span>
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
}) {
  const cells = formatCells(mode, record);
  return (
    <div
      className={`${COLS} cursor-pointer hover:bg-bg/40`}
      onClick={onToggle}
    >
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
      <span className="font-mono text-accent-blue">{cells.acq}</span>
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
          title="Move up"
          className="text-muted hover:text-primary disabled:opacity-30 text-[11px] px-1"
        >
          ▲
        </button>
        <button
          onClick={onMoveDown}
          disabled={!canMoveDown}
          title="Move down"
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
  const roam = roamIndLabel(record.roamingIndicator?.raw ?? 0);
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
