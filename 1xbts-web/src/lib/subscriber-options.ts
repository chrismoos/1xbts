import {
  NumberPlan,
  NumberType,
  SubscriberStatus,
} from "@/lib/proto/hlr/v1/service";

export const NUMBER_TYPE_OPTIONS: { value: NumberType; label: string }[] = [
  { value: NumberType.NUMBER_TYPE_NETWORK_SPECIFIC, label: "Network specific" },
  { value: NumberType.NUMBER_TYPE_UNKNOWN, label: "Unknown" },
  { value: NumberType.NUMBER_TYPE_INTERNATIONAL, label: "International" },
  { value: NumberType.NUMBER_TYPE_NATIONAL, label: "National" },
  { value: NumberType.NUMBER_TYPE_SUBSCRIBER, label: "Subscriber" },
  { value: NumberType.NUMBER_TYPE_ABBREVIATED, label: "Abbreviated" },
];

export const NUMBER_PLAN_OPTIONS: { value: NumberPlan; label: string }[] = [
  { value: NumberPlan.NUMBER_PLAN_ISDN_E164, label: "ISDN / E.164" },
  { value: NumberPlan.NUMBER_PLAN_UNKNOWN, label: "Unknown" },
  { value: NumberPlan.NUMBER_PLAN_DATA, label: "Data" },
  { value: NumberPlan.NUMBER_PLAN_TELEX, label: "Telex" },
  { value: NumberPlan.NUMBER_PLAN_PRIVATE, label: "Private" },
];

export const DEFAULT_NUMBER_TYPE = NumberType.NUMBER_TYPE_NETWORK_SPECIFIC;
export const DEFAULT_NUMBER_PLAN = NumberPlan.NUMBER_PLAN_ISDN_E164;

export function normalizeNumberType(value: NumberType | undefined | null): NumberType {
  if (
    value === undefined ||
    value === null ||
    value === NumberType.NUMBER_TYPE_UNSPECIFIED ||
    value === NumberType.UNRECOGNIZED
  ) {
    return DEFAULT_NUMBER_TYPE;
  }
  return value;
}

export function normalizeNumberPlan(value: NumberPlan | undefined | null): NumberPlan {
  if (
    value === undefined ||
    value === null ||
    value === NumberPlan.NUMBER_PLAN_UNSPECIFIED ||
    value === NumberPlan.UNRECOGNIZED
  ) {
    return DEFAULT_NUMBER_PLAN;
  }
  return value;
}

export type SubscriberStatusLabel = "active" | "suspended" | "disabled";

export function parseSubscriberStatus(value: unknown): SubscriberStatus {
  if (typeof value === "number") {
    return value as SubscriberStatus;
  }
  if (typeof value === "string") {
    switch (value.toLowerCase()) {
      case "active":
        return SubscriberStatus.SUBSCRIBER_STATUS_ACTIVE;
      case "suspended":
        return SubscriberStatus.SUBSCRIBER_STATUS_SUSPENDED;
      case "disabled":
        return SubscriberStatus.SUBSCRIBER_STATUS_DISABLED;
    }
  }
  return SubscriberStatus.SUBSCRIBER_STATUS_ACTIVE;
}

export function subscriberStatusLabel(
  value: SubscriberStatus | undefined | null
): SubscriberStatusLabel {
  switch (value) {
    case SubscriberStatus.SUBSCRIBER_STATUS_SUSPENDED:
      return "suspended";
    case SubscriberStatus.SUBSCRIBER_STATUS_DISABLED:
      return "disabled";
    default:
      return "active";
  }
}
