import { getBtsManagementClient, waitForBscReady } from "@/lib/grpc/client";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const encoder = new TextEncoder();
  const abort = new AbortController();

  request.signal.addEventListener("abort", () => abort.abort());

  const stream = new ReadableStream({
    async start(controller) {
      const send = (chunk: string) => {
        if (!abort.signal.aborted) {
          try {
            controller.enqueue(encoder.encode(chunk));
          } catch {
            abort.abort();
          }
        }
      };

      send("retry: 2000\n\n");

      try {
        await waitForBscReady();
        console.log("[radio-metrics] starting gRPC stream");
        const client = getBtsManagementClient();
        for await (const metrics of client.streamRadioMetrics(
          {},
          { signal: abort.signal }
        )) {
          if (abort.signal.aborted) break;
          send(`data: ${JSON.stringify(metrics)}\n\n`);
        }
        console.log("[radio-metrics] gRPC stream ended");
      } catch (err) {
        if (!abort.signal.aborted) {
          const msg = err instanceof Error ? err.message : "unknown error";
          console.log(`[radio-metrics] gRPC error: ${msg}`);
          send(`data: ${JSON.stringify({ error: msg })}\n\n`);
        } else {
          console.log("[radio-metrics] aborted");
        }
      }
      try {
        controller.close();
      } catch {
        // already closed
      }
    },
    cancel() {
      console.log("[radio-metrics] client disconnected");
      abort.abort();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}
