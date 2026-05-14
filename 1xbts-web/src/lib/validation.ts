export type ValidationResult = { ok: true } | { ok: false; error: string };

export const PHONE_MAX_DIGITS = 15;
export const IMSI_DIGITS = 15;

export function validatePhoneNumber(value: string): ValidationResult {
  if (value.length === 0) return { ok: false, error: "Phone number is required" };
  if (!/^\d+$/.test(value))
    return { ok: false, error: "Phone number must contain only digits 0–9" };
  if (value.length > PHONE_MAX_DIGITS)
    return {
      ok: false,
      error: `Phone number can be at most ${PHONE_MAX_DIGITS} digits (E.164)`,
    };
  return { ok: true };
}

export function validateImsi(value: string): ValidationResult {
  if (!/^\d+$/.test(value))
    return { ok: false, error: "IMSI must contain only digits 0–9" };
  if (value.length !== IMSI_DIGITS)
    return { ok: false, error: `IMSI must be exactly ${IMSI_DIGITS} digits` };
  return { ok: true };
}

export const RINGTONE_MAX_UPLOAD_BYTES = 4 * 1024 * 1024;
export const ACCEPTED_RINGTONE_MIME = [
  "audio/wav",
  "audio/wave",
  "audio/x-wav",
];

export function validateRingtoneFile(file: File): ValidationResult {
  if (file.size === 0) return { ok: false, error: "File is empty" };
  if (file.size > RINGTONE_MAX_UPLOAD_BYTES)
    return {
      ok: false,
      error: `File must be at most ${RINGTONE_MAX_UPLOAD_BYTES / (1024 * 1024)} MB`,
    };
  const name = file.name.toLowerCase();
  if (!name.endsWith(".wav"))
    return { ok: false, error: "File must be a .wav file" };
  return { ok: true };
}
