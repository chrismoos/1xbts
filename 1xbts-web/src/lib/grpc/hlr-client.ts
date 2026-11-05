import { createChannel } from "nice-grpc";
import { grpcClients, waitForService } from "./rich-client";
import { HlrServiceDefinition } from "../proto/hlr/v1/service";
import { coLocatedServiceAddress } from "./client";

const HLR_GRPC_ADDRESS =
  process.env.HLR_GRPC_ADDRESS || coLocatedServiceAddress(17019);

const hlrChannel = createChannel(HLR_GRPC_ADDRESS, undefined, {
  "grpc.initial_reconnect_backoff_ms": 100,
  "grpc.max_reconnect_backoff_ms": 1000,
});

export function getHlrClient() {
  return grpcClients.create(HlrServiceDefinition, hlrChannel);
}

export async function waitForHlrReady(timeoutMs = 1500) {
  await waitForService(hlrChannel, "/hlr.v1.HlrService", timeoutMs);
}
