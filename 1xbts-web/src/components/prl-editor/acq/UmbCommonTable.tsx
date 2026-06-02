import {
  PrlAcqRecord,
  PrlExtAcqRecord,
  PrlUmbAcqProfile,
} from "@/lib/proto/hlr/v1/service";

export function UmbCommonTableEditor({
  record,
  onPatch,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
}) {
  if (!("umbCommonTable" in record) || !record.umbCommonTable) return null;
  const entries = record.umbCommonTable.entries;

  const add = () =>
    onPatch((d) => {
      if ("umbCommonTable" in d && d.umbCommonTable)
        d.umbCommonTable.entries.push({
          umbAcqProfile: 0,
          fftSize: 0,
          cyclicPrefixLength: 0,
          numGuardSubcarriers: 0,
        });
    });

  const removeAt = (i: number) =>
    onPatch((d) => {
      if ("umbCommonTable" in d && d.umbCommonTable)
        d.umbCommonTable.entries.splice(i, 1);
    });

  const update = (i: number, patch: Partial<PrlUmbAcqProfile>) =>
    onPatch((d) => {
      if ("umbCommonTable" in d && d.umbCommonTable) {
        Object.assign(d.umbCommonTable.entries[i], patch);
      }
    });

  return (
    <div className="space-y-1">
      <div className="text-muted text-[11px]">
        UMB acquisition profiles{" "}
        <span className="text-dimmed">
          (profile 6b, FFT 4b, CP 3b, guard 7b)
        </span>
      </div>
      {entries.map((e, i) => (
        <div
          key={i}
          className="grid grid-cols-[auto_1fr_1fr_1fr_1fr_auto] gap-1 items-center"
        >
          <span className="font-mono text-dimmed text-[11px]">{i}</span>
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={e.umbAcqProfile}
            min={0}
            max={63}
            onChange={(ev) =>
              update(i, { umbAcqProfile: Number(ev.target.value) })
            }
            placeholder="profile"
          />
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={e.fftSize}
            min={0}
            max={15}
            onChange={(ev) =>
              update(i, { fftSize: Number(ev.target.value) })
            }
            placeholder="FFT size"
          />
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={e.cyclicPrefixLength}
            min={0}
            max={7}
            onChange={(ev) =>
              update(i, { cyclicPrefixLength: Number(ev.target.value) })
            }
            placeholder="CP"
          />
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={e.numGuardSubcarriers}
            min={0}
            max={127}
            onChange={(ev) =>
              update(i, { numGuardSubcarriers: Number(ev.target.value) })
            }
            placeholder="guard"
          />
          <button
            onClick={() => removeAt(i)}
            className="text-accent-red text-[11px] px-1 hover:underline"
          >
            ✕
          </button>
        </div>
      ))}
      <button
        onClick={add}
        className="text-accent-blue text-[11px] hover:underline"
      >
        + Add profile
      </button>
    </div>
  );
}
