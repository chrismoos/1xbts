import { getBtsManagementClient } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function GET() {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);
  try {
    const client = getBtsManagementClient();
    const result = await client.getBtsConfig({}, { signal: abort.signal });
    return Response.json(result);
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
