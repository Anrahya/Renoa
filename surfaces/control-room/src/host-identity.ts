// A fixed display palette: assignment survives agent renames and list reordering.
export const botPortraits = ["relay", "scout", "scribe", "beacon", "courier", "orbit",
  "forge", "patch", "vault", "lens", "signal", "tally"] as const;

export function genericPortrait(agentId: string): string {
  let hash = 2166136261;
  for (const character of agentId) hash = Math.imul(hash ^ character.charCodeAt(0), 16777619);
  return `/assets/identities/bots/${botPortraits[(hash >>> 0) % botPortraits.length]}.webp`;
}

const personalPortraits = new Map([
  ["arcee", "/assets/identities/arcee-prime.webp"],
  ["rc", "/assets/identities/arcee-prime.webp"],
  ["soundwave", "/assets/identities/soundwave-prime.webp"],
]);

export function portraitForAgent(agentId: string, name: string): string {
  return personalPortraits.get(name.trim().toLocaleLowerCase()) ?? genericPortrait(agentId);
}
