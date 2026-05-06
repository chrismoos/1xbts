import { getSmscClient } from "@/lib/grpc/smsc-client";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 5000);

  try {
    const { searchParams } = new URL(request.url);
    const limit = parseInt(searchParams.get("limit") || "50");
    const offset = parseInt(searchParams.get("offset") || "0");
    const state = searchParams.get("state") || undefined;

    const client = getSmscClient();
    const result = await client.listSmsSubmissions(
      { limit, offset, state, destinationNumber: undefined },
      { signal: abort.signal }
    );
    return Response.json({
      submissions: result.submissions,
      total: result.total,
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : "unknown error";
    return Response.json({ error: msg }, { status: 502 });
  } finally {
    clearTimeout(timeout);
  }
}
