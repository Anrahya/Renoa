import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import type { Controls } from "./host-controls";
import { readOwner, validCreatedAgent } from "./agent-onboarding-client";

interface Definition { id: string; operational: { instructions: string; model?: { provider: string; model: string; reasoning: string | null } }; tool_selection: { tools: string[]; revision: number } }
export function SavedAgentDefinition({ agentId, controls }: { agentId: string; controls: Controls }) {
  const [definition, setDefinition] = useState<Definition | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    readOwner(`/v1/host/agents/${encodeURIComponent(agentId)}`, value => {
      if (!validCreatedAgent(value) || (value as Definition).id !== agentId) throw new Error("The Host returned an incompatible agent definition.");
      return value as Definition;
    }, controller.signal).then(value => { setDefinition(value); setError(null); }).catch(error => { if (!controller.signal.aborted) setError(error.message); });
    return () => controller.abort();
  }, [agentId, controls.hostId, attempt]);
  return <div className="flex flex-col gap-4 text-sm">
    {definition ? <><dl className="grid gap-3 sm:grid-cols-2"><div><dt className="text-xs text-muted-foreground">Default model</dt><dd className="mt-1 break-all">{definition.operational.model ? `${definition.operational.model.provider} / ${definition.operational.model.model}` : "Host default"}</dd></div><div><dt className="text-xs text-muted-foreground">Reasoning</dt><dd className="mt-1">{definition.operational.model?.reasoning ?? "Model default"}</dd></div><div><dt className="text-xs text-muted-foreground">Machine access</dt><dd className="mt-1">{definition.tool_selection.tools.join(", ") || "None"}</dd></div></dl><details><summary className="font-medium">Standing instructions</summary><p className="mt-3 whitespace-pre-wrap break-words">{definition.operational.instructions}</p></details><p className="text-xs text-muted-foreground">Every agent has plugin management and discovery. Existing conversations can retain a different model selection.</p></> : !error && <p className="text-muted-foreground">Loading saved configuration…</p>}
    {error && <p role="status">{error}</p>}
    {error && <Button variant="outline" className="w-fit" onClick={() => setAttempt(value => value + 1)}>Retry configuration</Button>}
  </div>;
}
