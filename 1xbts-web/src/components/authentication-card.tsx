"use client";

import { useCallback, useEffect, useState } from "react";
import { akeyChecksum, randomAKey } from "@/lib/akey";
import { Card } from "@/components/card";
import { AuthenticationActivity } from "@/components/authentication-activity";

interface AuthState {
  credentialsProvisioned: boolean;
  ssdProvisioned: boolean;
  authCount: number;
  authFailureCount: number;
  lastAuthAt?: string;
}

const SAVE_BTN =
  "bg-accent-blue/20 border border-accent-blue/40 text-accent-blue px-3 py-1 rounded disabled:opacity-50";
const CLEAR_BTN =
  "bg-accent-red/10 border border-accent-red/40 text-accent-red px-3 py-1 rounded disabled:opacity-50";

export function AuthenticationCard({ subscriberId }: { subscriberId: string }) {
  const [state, setState] = useState<AuthState | null>(null);
  const [identities, setIdentities] = useState<{ imsi?: string; esn?: number }[]>([]);
  const [generationEsn, setGenerationEsn] = useState<number | null>(null);
  const [generated, setGenerated] = useState(false);
  const [saved, setSaved] = useState(false);
  const [entry, setEntry] = useState("");
  const [format, setFormat] = useState<"keypad" | "hex">("keypad");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const url = `/api/subscribers/${encodeURIComponent(subscriberId)}/authentication`;

  const load = useCallback(async () => {
    try {
      const res = await fetch(url);
      const data = await res.json();
      if (data.error) {
        setError(data.error);
      } else {
        setState(data.state);
        setGenerationEsn(data.generationEsn ?? null);
        setIdentities(data.identities ?? []);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "load failed");
    }
  }, [url]);

  useEffect(() => {
    load();
  }, [load]);

  const submit = async (aKey: string | null) => {
    const body = aKey != null && format === "hex" ? { aKeyHex: aKey } : { aKey };
    setBusy(true);
    setError(null);
    try {
      const res = await fetch(url, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      const data = await res.json();
      if (data.error) {
        setError(data.error);
      } else {
        if (aKey == null || !generated) {
          setEntry("");
          setGenerated(false);
        }
        setSaved(aKey != null && generated);
        await load();
      }
    } catch (error) {
      setError(error instanceof Error ? error.message : "Saving the A-key failed.");
    } finally {
      setBusy(false);
    }
  };

  const generate = () => {
    if (generationEsn == null) {
      setError("Configure an ESN on the subscriber's primary identity before generating an A-key.");
      return;
    }
    try {
      const key = randomAKey();
      setEntry(key + akeyChecksum(BigInt(key), generationEsn));
      setFormat("keypad");
      setGenerated(true);
      setSaved(false);
      setError(null);
    } catch {
      setError("A-key generation failed. A secure browser connection is required.");
    }
  };

  const digits = entry.replace(/\s+/g, "");
  const entryValid =
    format === "hex" ? /^(0x)?[0-9a-f]{1,16}$/i.test(digits) : /^\d{6,26}$/.test(digits);

  return (
    <Card title="Authentication">
      <div className="space-y-4 text-xs">
        <dl className="grid grid-cols-2 md:grid-cols-5 gap-3">
          <div>
            <dt className="text-muted">A-key</dt>
            <dd>{state?.credentialsProvisioned ? "Provisioned" : "Not set"}</dd>
          </div>
          <div>
            <dt className="text-muted">SSD</dt>
            <dd>
              {state?.ssdProvisioned
                ? "Established"
                : state?.credentialsProvisioned
                  ? "Zero until the next SSD update"
                  : "—"}
            </dd>
          </div>
          <div>
            <dt className="text-muted">COUNT</dt>
            <dd className="font-mono">{state ? state.authCount : "—"}</dd>
          </div>
          <div>
            <dt className="text-muted">Failures since last success</dt>
            <dd className="font-mono">{state ? state.authFailureCount : "—"}</dd>
          </div>
          <div>
            <dt className="text-muted">Last authenticated</dt>
            <dd>
              {state?.lastAuthAt
                ? new Date(state.lastAuthAt).toLocaleString()
                : "Never"}
            </dd>
          </div>
        </dl>
        <section className="space-y-2">
          <h4 className="font-medium">Recent activity</h4>
          <AuthenticationActivity
            imsis={identities.flatMap((identity) => (identity.imsi ? [identity.imsi] : []))}
            esn={identities.find((identity) => identity.esn != null)?.esn}
          />
        </section>
        <section className="space-y-2">
          <h4 className="font-medium">A-key entry</h4>
          <p className="text-muted">
            Keypad form: up to 20 digits of A-key followed by 6 check digits,
            which are verified against the identity&apos;s ESN. Hex form: the
            64-bit A-key as up to 16 hex digits, with no check. Entering an
            A-key resets the SSD to zero, as the handset does, so the next
            authenticated call runs an SSD update.
          </p>
          <p className="text-muted">
            OTASP (<span className="font-mono">*228</span>) generates an A-key only when none
            is on file. Clear the A-key to allow a new one.
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <select
              value={format}
              onChange={(e) => {
                setFormat(e.target.value as "keypad" | "hex");
                setEntry("");
                setGenerated(false);
                setSaved(false);
              }}
              className="glass-input"
              aria-label="A-key format"
            >
              <option value="keypad">Keypad (decimal + check digits)</option>
              <option value="hex">Hex</option>
            </select>
            <input
              type="text"
              inputMode={format === "hex" ? "text" : "numeric"}
              value={entry}
              onChange={(e) => {
                setEntry(e.target.value);
                setGenerated(false);
                setSaved(false);
              }}
              placeholder={format === "hex" ? "C442F56BE9E17158" : "14 1421 3562 3730 9504 8808 6500"}
              className="glass-input font-mono w-80 max-w-full"
              aria-label={format === "hex" ? "A-key in hex" : "A-key with check digits"}
            />
            <button
              type="button"
              className="border border-white/20 px-3 py-1 rounded disabled:opacity-50 disabled:cursor-not-allowed"
              disabled={busy || generationEsn == null}
              aria-describedby="akey-generation-help"
              onClick={generate}
            >
              Generate
            </button>
            <button
              type="button"
              className={SAVE_BTN}
              disabled={busy || !entryValid}
              onClick={() => submit(digits)}
            >
              Set A-key
            </button>
            <button
              type="button"
              className={CLEAR_BTN}
              disabled={busy || !state?.credentialsProvisioned}
              onClick={() => submit(null)}
            >
              Clear
            </button>
          </div>
          <p id="akey-generation-help" className="text-muted">
            {generationEsn == null
              ? "Configure an ESN on the subscriber's primary identity to enable generation."
              : `Generation uses configured ESN ${generationEsn.toString(16).toUpperCase().padStart(8, "0")}.`}
          </p>
          {generated && (
            <p role="status" className="text-muted">
              {saved
                ? "A-key saved. Copy the keypad entry above to program the handset."
                : "A-key generated with check digits. Select Set A-key to save it."}
            </p>
          )}
          {error && <p className="text-accent-red">{error}</p>}
        </section>
      </div>
    </Card>
  );
}
