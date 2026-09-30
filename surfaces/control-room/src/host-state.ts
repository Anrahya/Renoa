/** One recorded event for an agent, placed on a day axis. */
export type DayMark = { at: number; state: AgentState };

// The four states the whole control room reports, ordered by what the owner can
// do about them. An agent waiting on the owner is the healthy, actionable case
// and wears gold; only work that actually broke wears red.
export type AgentState = "needs-you" | "failed" | "running" | "idle";
export const agentStates: AgentState[] = ["needs-you", "failed", "running", "idle"];
export const stateRank: Record<AgentState, number> = { "needs-you": 0, failed: 1, running: 2, idle: 3 };
export const stateLabel: Record<AgentState, string> = {
  "needs-you": "Needs you", failed: "Failed", running: "Unfinished", idle: "No unfinished work",
};
export const emptyCounts = (): Record<AgentState, number> => ({ "needs-you": 0, failed: 0, running: 0, idle: 0 });

// A record waiting on the owner is a question, not a fault. A record that was
// interrupted is work that broke, and the two must never share a colour.
// Note the live and example summary paths currently reach "interrupted" from
// different facts, so this is the one place the two states are separated.
export function toneState(tone: string | undefined, synthetic = false): AgentState {
  if (synthetic) return "idle";
  switch (tone) {
    case "waiting": return "needs-you";
    case "interrupted": return "failed";
    case "pending": return "running";
    default: return "idle";
  }
}
