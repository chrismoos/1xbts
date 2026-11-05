import { createChannel } from "nice-grpc";
import { grpcClients, waitForService } from "./rich-client";
import { SmscServiceDefinition } from "../proto/smsc/v1/service";
import { coLocatedServiceAddress } from "./client";

const SMSC_GRPC_ADDRESS =
  process.env.SMSC_GRPC_ADDRESS || coLocatedServiceAddress(17020);

const smscChannel = createChannel(SMSC_GRPC_ADDRESS, undefined, {
  "grpc.initial_reconnect_backoff_ms": 100,
  "grpc.max_reconnect_backoff_ms": 1000,
});

export function getSmscClient() {
  return grpcClients.create(SmscServiceDefinition, smscChannel);
}

export async function waitForSmscReady(timeoutMs = 1500) {
  await waitForService(smscChannel, "/smsc.v1.SmscService", timeoutMs);
}
