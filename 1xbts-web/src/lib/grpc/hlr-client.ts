import { createChannel, createClient, waitForChannelReady } from "nice-grpc";
import { HlrServiceDefinition } from "../proto/hlr/v1/service";
import { coLocatedServiceAddress } from "./client";

const HLR_GRPC_ADDRESS =
  process.env.HLR_GRPC_ADDRESS || coLocatedServiceAddress(17019);

const hlrChannel = createChannel(HLR_GRPC_ADDRESS, undefined, {
  "grpc.initial_reconnect_backoff_ms": 100,
  "grpc.max_reconnect_backoff_ms": 1000,
});

export function getHlrClient() {
  return createClient(HlrServiceDefinition, hlrChannel);
}

export async function waitForHlrReady(timeoutMs = 1500) {
  await waitForChannelReady(hlrChannel, new Date(Date.now() + timeoutMs));
}
