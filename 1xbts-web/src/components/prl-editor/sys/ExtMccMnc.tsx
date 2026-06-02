import { PrlExtSysRecord } from "@/lib/proto/hlr/v1/service";
import {
  PREF_NEG_OPTIONS,
  PRIORITY_OPTIONS,
  MCC_MNC_SUBTYPE_OPTIONS,
} from "@/lib/prl-options";
import { EnumSelect } from "../shared/EnumSelect";
import { TextInput } from "../shared/NumericInput";
import { HexBytesInput } from "../shared/HexBytesInput";
import { AcqIndexPicker } from "../shared/AcqIndexPicker";
import { emptyMccMnc, MccMncSubtypeKey } from "../builders";
import { ErrorMap } from "../validation";
import { EditorMode } from "../state";
import { AcqPickerContext } from "./SysRowEditor";

function currentSubtype(r: PrlExtSysRecord): MccMncSubtypeKey | null {
  if (!r.mccMnc) return null;
  if (r.mccMnc.subtype000) return "subtype000";
  if (r.mccMnc.subtype001) return "subtype001";
  if (r.mccMnc.subtype010) return "subtype010";
  if (r.mccMnc.subtype011) return "subtype011";
  return null;
}

export function ExtMccMncEditor({
  record,
  onPatch,
  mode,
  acq,
  errors,
  errorPrefix,
}: {
  record: PrlExtSysRecord;
  onPatch: (mutator: (draft: PrlExtSysRecord) => void) => void;
  mode: EditorMode;
  acq: AcqPickerContext;
  errors: ErrorMap;
  errorPrefix: string;
}) {
  const sub = currentSubtype(record);

  return (
    <div className="space-y-2">
      <AcqIndexPicker
        value={record.acqIndex}
        onChange={(v) => onPatch((d) => void (d.acqIndex = v))}
        mode={mode}
        acqRecords={acq.acqRecords}
        patchAcq={acq.patchAcq}
        errors={errors}
        errorPrefix={errorPrefix}
        error={errors.get(`${errorPrefix}.acqIndex`)}
      />
      <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
        <EnumSelect
          label="PREF_NEG"
          value={record.prefNeg}
          options={PREF_NEG_OPTIONS}
          onChange={(v) => onPatch((d) => void (d.prefNeg = v))}
        />
        <EnumSelect
          label="PRI"
          value={record.priority}
          options={PRIORITY_OPTIONS}
          onChange={(v) => onPatch((d) => void (d.priority = v))}
        />
      </div>

      <label className="block">
        <span className="text-muted text-[11px]">SYS_RECORD_SUBTYPE</span>
        <select
          className="block w-full mt-0.5 bg-bg border border-border rounded px-2 py-1 text-xs"
          value={sub ?? ""}
          onChange={(e) => {
            const next = e.target.value as MccMncSubtypeKey;
            onPatch((d) => {
              if (d.mccMnc) Object.assign(d.mccMnc, emptyMccMnc(next));
            });
          }}
        >
          {MCC_MNC_SUBTYPE_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>

      {sub && <MccMncBody record={record} onPatch={onPatch} subtype={sub} errors={errors} errorPrefix={errorPrefix} />}
    </div>
  );
}

function MccMncBody({
  record,
  onPatch,
  subtype,
  errors,
  errorPrefix,
}: {
  record: PrlExtSysRecord;
  onPatch: (mutator: (draft: PrlExtSysRecord) => void) => void;
  subtype: MccMncSubtypeKey;
  errors: ErrorMap;
  errorPrefix: string;
}) {
  if (subtype === "subtype000") {
    const b = record.mccMnc!.subtype000!;
    return (
      <div className="grid grid-cols-2 gap-2">
        <TextInput
          label="MCC"
          value={b.mcc}
          onChange={(v) =>
            onPatch((d) => {
              if (d.mccMnc?.subtype000) d.mccMnc.subtype000.mcc = v;
            })
          }
          maxLength={3}
          error={errors.get(`${errorPrefix}.mccMnc.subtype000.mcc`)}
        />
        <TextInput
          label="MNC"
          value={b.mnc}
          onChange={(v) =>
            onPatch((d) => {
              if (d.mccMnc?.subtype000) d.mccMnc.subtype000.mnc = v;
            })
          }
          maxLength={3}
          error={errors.get(`${errorPrefix}.mccMnc.subtype000.mnc`)}
        />
      </div>
    );
  }
  if (subtype === "subtype001") {
    const b = record.mccMnc!.subtype001!;
    return (
      <div className="space-y-2">
        <div className="grid grid-cols-2 gap-2">
          <TextInput
            label="MCC"
            value={b.mcc}
            onChange={(v) =>
              onPatch((d) => {
                if (d.mccMnc?.subtype001) d.mccMnc.subtype001.mcc = v;
              })
            }
            maxLength={3}
          />
          <TextInput
            label="MNC"
            value={b.mnc}
            onChange={(v) =>
              onPatch((d) => {
                if (d.mccMnc?.subtype001) d.mccMnc.subtype001.mnc = v;
              })
            }
            maxLength={3}
          />
        </div>
        <ListField<number>
          label="SIDs (16-bit each)"
          items={b.sids}
          create={() => 0}
          render={(v, set) => (
            <input
              type="number"
              className="flex-1 bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
              value={v}
              onChange={(e) => set(Number(e.target.value))}
              min={0}
              max={0xffff}
            />
          )}
          onChange={(next) =>
            onPatch((d) => {
              if (d.mccMnc?.subtype001) d.mccMnc.subtype001.sids = next;
            })
          }
        />
      </div>
    );
  }
  if (subtype === "subtype010") {
    const b = record.mccMnc!.subtype010!;
    return (
      <div className="space-y-2">
        <div className="grid grid-cols-2 gap-2">
          <TextInput
            label="MCC"
            value={b.mcc}
            onChange={(v) =>
              onPatch((d) => {
                if (d.mccMnc?.subtype010) d.mccMnc.subtype010.mcc = v;
              })
            }
            maxLength={3}
          />
          <TextInput
            label="MNC"
            value={b.mnc}
            onChange={(v) =>
              onPatch((d) => {
                if (d.mccMnc?.subtype010) d.mccMnc.subtype010.mnc = v;
              })
            }
            maxLength={3}
          />
        </div>
        <ListField<{ sid: number; nid: number }>
          label="(SID, NID) pairs"
          items={b.pairs}
          create={() => ({ sid: 0, nid: 0 })}
          render={(v, set) => (
            <div className="flex gap-1 flex-1">
              <input
                type="number"
                className="flex-1 bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
                value={v.sid}
                onChange={(e) => set({ ...v, sid: Number(e.target.value) })}
                min={0}
                max={0xffff}
                placeholder="SID"
              />
              <input
                type="number"
                className="flex-1 bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
                value={v.nid}
                onChange={(e) => set({ ...v, nid: Number(e.target.value) })}
                min={0}
                max={0xffff}
                placeholder="NID"
              />
            </div>
          )}
          onChange={(next) =>
            onPatch((d) => {
              if (d.mccMnc?.subtype010) d.mccMnc.subtype010.pairs = next;
            })
          }
        />
      </div>
    );
  }
  // subtype011
  const b = record.mccMnc!.subtype011!;
  return (
    <div className="space-y-2">
      <div className="grid grid-cols-2 gap-2">
        <TextInput
          label="MCC"
          value={b.mcc}
          onChange={(v) =>
            onPatch((d) => {
              if (d.mccMnc?.subtype011) d.mccMnc.subtype011.mcc = v;
            })
          }
          maxLength={3}
        />
        <TextInput
          label="MNC"
          value={b.mnc}
          onChange={(v) =>
            onPatch((d) => {
              if (d.mccMnc?.subtype011) d.mccMnc.subtype011.mnc = v;
            })
          }
          maxLength={3}
        />
      </div>
      <ListField<{ subnetLengthBits: number; subnetIdHex: string }>
        label="HRPD subnets"
        items={b.subnets}
        create={() => ({ subnetLengthBits: 0, subnetIdHex: "" })}
        render={(v, set) => (
          <div className="flex gap-1 flex-1 items-center">
            <input
              type="number"
              className="w-16 bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
              value={v.subnetLengthBits}
              onChange={(e) => {
                const bits = Number(e.target.value);
                const want = Math.ceil(bits / 8) * 2;
                let id = v.subnetIdHex;
                if (id.length < want) id = id + "0".repeat(want - id.length);
                else if (id.length > want) id = id.slice(0, want);
                set({ subnetLengthBits: bits, subnetIdHex: id });
              }}
              min={0}
              max={128}
              title="bits"
            />
            <HexBytesInput
              lengthBits={v.subnetLengthBits}
              value={v.subnetIdHex}
              onChange={(hex) => set({ ...v, subnetIdHex: hex })}
            />
          </div>
        )}
        onChange={(next) =>
          onPatch((d) => {
            if (d.mccMnc?.subtype011) d.mccMnc.subtype011.subnets = next;
          })
        }
      />
    </div>
  );
}

function ListField<T>({
  label,
  items,
  create,
  render,
  onChange,
}: {
  label: string;
  items: T[];
  create: () => T;
  render: (item: T, set: (next: T) => void) => React.ReactNode;
  onChange: (next: T[]) => void;
}) {
  const update = (i: number, value: T) =>
    onChange(items.map((x, j) => (j === i ? value : x)));
  return (
    <div className="space-y-1">
      <div className="text-muted text-[11px]">{label}</div>
      {items.map((item, i) => (
        <div key={i} className="flex gap-1 items-center">
          <span className="font-mono text-dimmed text-[11px] w-4">{i}</span>
          {render(item, (next) => update(i, next))}
          <button
            onClick={() => onChange(items.filter((_, j) => j !== i))}
            className="text-accent-red text-[11px] px-1 hover:underline"
          >
            ✕
          </button>
        </div>
      ))}
      <button
        onClick={() => onChange([...items, create()])}
        disabled={items.length >= 15}
        className="text-accent-blue text-[11px] hover:underline disabled:opacity-50"
      >
        + Add
      </button>
    </div>
  );
}
