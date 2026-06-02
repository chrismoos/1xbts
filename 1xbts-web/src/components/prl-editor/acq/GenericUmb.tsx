import {
  PrlAcqRecord,
  PrlExtAcqRecord,
  PrlUmbBlock,
} from "@/lib/proto/hlr/v1/service";

export function GenericUmbEditor({
  record,
  onPatch,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
}) {
  if (!("genericUmb" in record) || !record.genericUmb) return null;
  const blocks = record.genericUmb.blocks;

  const add = () =>
    onPatch((d) => {
      if ("genericUmb" in d && d.genericUmb)
        d.genericUmb.blocks.push({
          bandClass: 0,
          channelNumber: 0,
          umbAcqTableProfile: 0,
        });
    });

  const removeAt = (i: number) =>
    onPatch((d) => {
      if ("genericUmb" in d && d.genericUmb) d.genericUmb.blocks.splice(i, 1);
    });

  const update = (i: number, patch: Partial<PrlUmbBlock>) =>
    onPatch((d) => {
      if ("genericUmb" in d && d.genericUmb) {
        Object.assign(d.genericUmb.blocks[i], patch);
      }
    });

  return (
    <div className="space-y-1">
      <div className="text-muted text-[11px]">
        UMB blocks{" "}
        <span className="text-dimmed">
          (band 8b, channel 16b, profile 6b; 0x3F = ignore common table)
        </span>
      </div>
      {blocks.map((b, i) => (
        <div
          key={i}
          className="grid grid-cols-[auto_1fr_1fr_1fr_auto] gap-1 items-center"
        >
          <span className="font-mono text-dimmed text-[11px]">{i}</span>
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={b.bandClass}
            min={0}
            max={255}
            onChange={(ev) => update(i, { bandClass: Number(ev.target.value) })}
            placeholder="Band class"
          />
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={b.channelNumber}
            min={0}
            max={65535}
            onChange={(ev) =>
              update(i, { channelNumber: Number(ev.target.value) })
            }
            placeholder="Channel"
          />
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={b.umbAcqTableProfile}
            min={0}
            max={63}
            onChange={(ev) =>
              update(i, { umbAcqTableProfile: Number(ev.target.value) })
            }
            placeholder="profile"
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
        disabled={blocks.length >= 63}
        className="text-accent-blue text-[11px] hover:underline disabled:opacity-50"
      >
        + Add block
      </button>
    </div>
  );
}
