export interface ApiErrorField {
  field: string;
  description: string;
}

// The JSON body every API route returns on failure. `error` is the complete
// text to show.
export interface ApiErrorBody {
  error: string;
  title?: string;
  message: string;
  code: string;
  // Without the ERROR_REASON_ prefix.
  reason?: string;
  service?: string;
  metadata?: Record<string, string>;
  fields?: ApiErrorField[];
  detail?: string;
}

export function isApiErrorBody(value: unknown): value is ApiErrorBody {
  return Boolean(
    value && typeof value === "object" && typeof (value as { error?: unknown }).error === "string",
  );
}
