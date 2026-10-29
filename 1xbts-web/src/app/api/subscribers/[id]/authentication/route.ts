// PUT accepts `{ aKey: string | null }` or `{ aKeyHex: string }`. `aKey` is the 26-digit
// keypad form, the A-key then its check digits, which the HLR verifies against
// the identity's ESN. `aKeyHex` is the 64-bit A-key in hex with no check
// digits. `aKey: null` clears the A-key and SSD.

import { grpcErrorMessage, grpcErrorStatus } from "@/lib/grpc/client";
import { getHlrClient, waitForHlrReady } from "@/lib/grpc/hlr-client";
import { AuthAlgorithm, MobileIdentityKey } from "@/lib/proto/hlr/v1/service";

export const dynamic = "force-dynamic";

type Ctx = { params: Promise<{ id: string }> };

const A_KEY_ENTRY = /^\d{6,26}$/;
const A_KEY_HEX = /^(0x)?[0-9a-f]{1,16}$/i;
const A_KEY_OCTETS = 8;
const REQUEST_TIMEOUT_MS = 5_000;

/** The A-key as 8 big-endian octets. */
function aKeyFromHex(hex: string): Uint8Array {
  const digits = hex.replace(/^0x/i, "").padStart(A_KEY_OCTETS * 2, "0");
  return Uint8Array.from({ length: A_KEY_OCTETS }, (_, i) =>
    parseInt(digits.slice(i * 2, i * 2 + 2), 16)
  );
}

async function primaryIdentity(
  id: string,
  signal: AbortSignal
): Promise<MobileIdentityKey | null> {
  const client = getHlrClient();
  const result = await client.getSubscriber({ subscriberId: id }, { signal });
  const identity = result.primaryIdentity ?? result.identities[0];
  if (!identity) return null;
  return { imsi: identity.imsi, esn: identity.esn, meid: identity.meid };
}

export async function GET(_request: Request, ctx: Ctx) {
  const { id } = await ctx.params;
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), REQUEST_TIMEOUT_MS);
  try {
    await waitForHlrReady();
    const subscriber = await getHlrClient().getSubscriber(
      { subscriberId: id },
      { signal: abort.signal }
    );
    const identities = subscriber.identities.map((identity) => ({
      imsi: identity.imsi,
      esn: identity.esn,
    }));
    const identity = await primaryIdentity(id, abort.signal);
    if (!identity) return Response.json({ state: null, identities });
    const state = await getHlrClient()
      .getSubscriberAuthState({ identity }, { signal: abort.signal })
      .catch((err) => {
        if (grpcErrorStatus(err) === 404) return null;
        throw err;
      });
    return Response.json({ state, identities, generationEsn: identity.esn ?? null });
  } catch (err) {
    return Response.json(
      { error: grpcErrorMessage(err) },
      { status: grpcErrorStatus(err) }
    );
  } finally {
    clearTimeout(timeout);
  }
}

export async function PUT(request: Request, ctx: Ctx) {
  const { id } = await ctx.params;
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), REQUEST_TIMEOUT_MS);
  try {
    const body = await request.json().catch(() => ({}));
    const hex =
      typeof body.aKeyHex === "string" ? body.aKeyHex.replace(/\s+/g, "") : "";
    if (hex && !A_KEY_HEX.test(hex)) {
      return Response.json(
        { error: "A-key hex must be up to 16 hex digits" },
        { status: 400 }
      );
    }
    const raw =
      typeof body.aKey === "string" ? body.aKey.replace(/\s+/g, "") : "";
    if (raw && !A_KEY_ENTRY.test(raw)) {
      return Response.json(
        { error: "A-key must be 6 to 26 digits including the 6 check digits" },
        { status: 400 }
      );
    }
    await waitForHlrReady();
    const identity = await primaryIdentity(id, abort.signal);
    if (!identity) {
      return Response.json(
        { error: "subscriber has no identity" },
        { status: 400 }
      );
    }
    await getHlrClient().setSubscriberAKey(
      {
        identity,
        aKey: hex ? aKeyFromHex(hex) : undefined,
        ssdA: undefined,
        ssdB: undefined,
        authAlgo:
          raw || hex ? AuthAlgorithm.AUTH_ALGORITHM_CAVE : AuthAlgorithm.AUTH_ALGORITHM_NONE,
        aKeyDigits: raw || undefined,
      },
      { signal: abort.signal }
    );
    return Response.json({ ok: true });
  } catch (err) {
    return Response.json(
      { error: grpcErrorMessage(err) },
      { status: grpcErrorStatus(err) }
    );
  } finally {
    clearTimeout(timeout);
  }
}
