// A fixed display palette: assignment survives agent renames and list reordering.
export const botPortraits = ["relay", "scout", "scribe", "beacon", "courier", "orbit",
  "forge", "patch", "vault", "lens", "signal", "tally"] as const;

export function genericPortrait(agentId: string): string {
  let hash = 2166136261;
  for (const character of agentId) hash = Math.imul(hash ^ character.charCodeAt(0), 16777619);
  return `/assets/identities/bots/${botPortraits[(hash >>> 0) % botPortraits.length]}.webp`;
}
