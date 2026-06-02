import {
  PrlAcqRecord,
  PrlExtAcqRecord,
  PrlBandClassChannel,
} from "@/lib/proto/hlr/v1/service";

export function Generic1xIs95Editor({
  record,
  onPatch,
}: {
  record: PrlAcqRecord | PrlExtAcqRecord;
  onPatch: (mutator: (draft: PrlAcqRecord | PrlExtAcqRecord) => void) => void;
}) {
  if (!("generic1xIs95" in record) || !record.generic1xIs95) return null;
  const entries = record.generic1xIs95.entries;

  const add = () =>
    onPatch((d) => {
      if ("generic1xIs95" in d && d.generic1xIs95)
        d.generic1xIs95.entries.push({ bandClass: 0, channelNumber: 0 });
    });

  const removeAt = (i: number) =>
    onPatch((d) => {
      if ("generic1xIs95" in d && d.generic1xIs95)
        d.generic1xIs95.entries.splice(i, 1);
    });

  const update = (i: number, patch: Partial<PrlBandClassChannel>) =>
    onPatch((d) => {
      if ("generic1xIs95" in d && d.generic1xIs95) {
        Object.assign(d.generic1xIs95.entries[i], patch);
      }
    });

  return <BandClassChannelList entries={entries} add={add} remove={removeAt} update={update} label="1x/IS-95 entries" />;
}

export function BandClassChannelList({
  entries,
  add,
  remove,
  update,
  label,
}: {
  entries: PrlBandClassChannel[];
  add: () => void;
  remove: (i: number) => void;
  update: (i: number, patch: Partial<PrlBandClassChannel>) => void;
  label: string;
}) {
  return (
    <div className="space-y-1">
      <div className="text-muted text-[11px]">
        {label}{" "}
        <span className="text-dimmed">
          ({entries.length} pairs; band class 5 bits, channel 11 bits)
        </span>
      </div>
      {entries.map((e, i) => (
        <div key={i} className="grid grid-cols-[auto_1fr_1fr_auto] gap-1 items-center">
          <span className="font-mono text-dimmed text-[11px]">{i}</span>
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={e.bandClass}
            min={0}
            max={31}
            onChange={(ev) => update(i, { bandClass: Number(ev.target.value) })}
            placeholder="Band class"
          />
          <input
            type="number"
            className="bg-bg border border-border rounded px-2 py-0.5 text-xs font-mono"
            value={e.channelNumber}
            min={0}
            max={2047}
            onChange={(ev) =>
              update(i, { channelNumber: Number(ev.target.value) })
            }
            placeholder="Channel"
          />
          <button
            onClick={() => remove(i)}
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
        + Add pair
      </button>
    </div>
  );
}
