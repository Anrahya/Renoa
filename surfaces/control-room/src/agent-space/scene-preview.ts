import type { Agent } from "../host-contract";
import type { SpaceAgent } from "./scene";
import { capabilityPlugins, pluginCapabilities } from "../agent-work-preview/configuration-model";
// Explicit design fixtures, not authority inferred from creation provenance.
const exampleManagement = new Map([
  ["42357f5e-ae1f-0802-5218-d7f65a043086", "20340f86-7f10-4c52-8757-c3124d9af0e1"],
  ["00000000-0000-0000-0000-000000000003", "00000000-0000-0000-0000-000000000002"],
]);
export function previewManager(agent: Agent, agents: Agent[]): string | undefined {
  const manager = exampleManagement.get(agent.id);
  return agents.some(candidate => candidate.id === manager) ? manager : undefined;
}

const groupNames = ["Atlas", "Beacon", "Orbit", "Relay", "Scout", "Cedar"];
export function expandScene(source: SpaceAgent[]): SpaceAgent[] {
  const agents = [...source];

    const count = Math.max(0, 50 - agents.length);
    for (let i = 0; i < count; i++) {
      const group = Math.floor(i / 8);
      agents.push({ id: `space-example-${i}`, name: i % 8 === 0 ? groupNames[group % groupNames.length]! : `${groupNames[group % groupNames.length]} ${i % 8}`,
        originalName: "Example agent", synthetic: true, managerId: i % 8 === 0 ? undefined : `space-example-${group * 8}`,
        capabilityIds: i % 3 === 0 ? ["read", "search", "mail-read"] : i % 3 === 1 ? ["read", "web", "research"] : ["web", "research", "writing"] });
    }

  return agents;
}
export const previewPlugins = capabilityPlugins.map(plugin => ({ id: plugin.id, name: plugin.name, capabilityIds: pluginCapabilities(plugin).map(item => item.id) }));
