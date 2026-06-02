import { PrlExtSysRecord } from "@/lib/proto/hlr/v1/service";
import {
  PREF_NEG_OPTIONS,
  PRIORITY_OPTIONS,
} from "@/lib/prl-options";
import { EnumSelect } from "../shared/EnumSelect";
import { NumericInput } from "../shared/NumericInput";
import { HexBytesInput } from "../shared/HexBytesInput";
import { AcqIndexPicker } from "../shared/AcqIndexPicker";
import { RoamingIndicatorSelect } from "../shared/RoamingIndicatorSelect";
import { ErrorMap } from "../validation";
import { EditorMode } from "../state";
import { AcqPickerContext } from "./SysRowEditor";

export function ExtHrpdEditor({
  record,
  onPatch,
  mode,
  acq,
  subnetCount,
  errors,
  errorPrefix,
}: {
  record: PrlExtSysRecord;
  onPatch: (mutator: (draft: PrlExtSysRecord) => void) => void;
  mode: EditorMode;
  acq: AcqPickerContext;
  subnetCount: number;
  errors: ErrorMap;
  errorPrefix: string;
}) {
  const hrpd = record.hrpd!;

  return (
    <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
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
      <label className="block">
        <span className="text-muted text-[11px]">GEO (same as prev)</span>
        <div className="mt-1">
          <input
            type="checkbox"
            checked={record.sameGeoAsPrev}
            onChange={(e) =>
              onPatch((d) => void (d.sameGeoAsPrev = e.target.checked))
            }
          />
        </div>
      </label>

      <label className="block col-span-full border-t border-border/30 pt-2 mt-2">
        <span className="text-muted text-[11px]">HRPD subnet</span>
      </label>
      <NumericInput
        label="SUBNET_LSB_LENGTH (bits)"
        min={0}
        max={127}
        value={hrpd.subnetLsbLengthBits}
        onChange={(v) =>
          onPatch((d) => {
            if (!d.hrpd) return;
            d.hrpd.subnetLsbLengthBits = v;
            // Trim/pad hex to match new length
            const want = Math.ceil(v / 8) * 2;
            const cur = d.hrpd.subnetLsbHex;
            if (cur.length < want) d.hrpd.subnetLsbHex = cur + "0".repeat(want - cur.length);
            else if (cur.length > want) d.hrpd.subnetLsbHex = cur.slice(0, want);
          })
        }
        error={errors.get(`${errorPrefix}.hrpd.subnetLsbLengthBits`)}
      />
      <HexBytesInput
        label="SUBNET_LSB (hex)"
        lengthBits={hrpd.subnetLsbLengthBits}
        value={hrpd.subnetLsbHex}
        onChange={(v) =>
          onPatch((d) => {
            if (d.hrpd) d.hrpd.subnetLsbHex = v;
          })
        }
        error={errors.get(`${errorPrefix}.hrpd.subnetLsbHex`)}
      />
      <label className="block">
        <span className="text-muted text-[11px]">
          SUBNET_COMMON_INCLUDED
        </span>
        <div className="mt-1">
          <input
            type="checkbox"
            checked={hrpd.subnetCommonIncluded}
            onChange={(e) =>
              onPatch((d) => {
                if (!d.hrpd) return;
                d.hrpd.subnetCommonIncluded = e.target.checked;
                if (!e.target.checked) d.hrpd.subnetCommonOffset = undefined;
                else if (d.hrpd.subnetCommonOffset == null)
                  d.hrpd.subnetCommonOffset = 0;
              })
            }
          />
        </div>
      </label>
      {hrpd.subnetCommonIncluded && (
        <NumericInput
          label="SUBNET_COMMON_OFFSET"
          hint={`0–${Math.max(0, subnetCount - 1)} (Common Subnet row index)`}
          min={0}
          max={Math.max(0, subnetCount - 1)}
          value={hrpd.subnetCommonOffset ?? 0}
          onChange={(v) =>
            onPatch((d) => {
              if (d.hrpd) d.hrpd.subnetCommonOffset = v;
            })
          }
          error={errors.get(`${errorPrefix}.hrpd.subnetCommonOffset`)}
        />
      )}

      <RoamingIndicatorSelect
        label="ROAM_IND"
        value={record.roamingIndicator?.raw ?? 0}
        onChange={(v) =>
          onPatch((d) => {
            if (!d.roamingIndicator) d.roamingIndicator = { raw: 0, kind: 0 };
            d.roamingIndicator.raw = v;
          })
        }
      />
    </div>
  );
}
