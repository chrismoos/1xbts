export function radioConfigName(rc?: number): string | null {
  return rc != null && rc > 0 ? `RC${rc}` : null;
}

export function radioConfigPairName(forwardRc?: number, reverseRc?: number): string | null {
  const forward = radioConfigName(forwardRc);
  const reverse = radioConfigName(reverseRc);
  if (forward && reverse) return `${forward}/${reverse}`;
  if (forward) return `Fwd ${forward}`;
  if (reverse) return `Rev ${reverse}`;
  return null;
}
