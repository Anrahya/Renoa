import { describe, expect, it } from "vitest";
import { createScene, pluginMembers, previewManager, regionPath, type SpaceAgent } from "./scene";
import type { Agent } from "../host-contract";
import type { DirectorySummary, DirectoryTone } from "../host-agent-directory-model";

const summary = (tone: DirectoryTone): DirectorySummary => ({
  tone, status: "Recorded state", title: "Recorded work", detail: "", workHref: "/host",
  automated: false, automationCount: 0, day: [], lastAt: null,
  next: { title: "No scheduled work", detail: "", href: "/host" },
});

const rc: Agent = { id: "20340f86-7f10-4c52-8757-c3124d9af0e1", name: "Arcee", created_by: null, preset_id: "renoa.personal.arcee.v3" };
const desk: Agent = { id: "42357f5e-ae1f-0802-5218-d7f65a043086", name: "X Desk", created_by: rc.id, preset_id: "renoa.coding.alpha.v3" };
const sound: Agent = { id: "c8a63c3b-166d-45a0-9324-2b9db6f3d2df", name: "Soundwave", created_by: rc.id, preset_id: "renoa.general.v1" };
const agents = [rc, desk, sound];
const source: SpaceAgent[] = agents.map(agent => ({ id: agent.id, name: agent.name, originalName: agent.name,
  managerId: previewManager(agent, agents), capabilityIds: agent.id === sound.id ? ["mail-read"] : ["read", "search"] }));

describe("Agent-space preview boundaries", () => {
  it("uses the explicit example management relationship, not creation provenance or names", () => {
    expect(previewManager(desk, agents)).toBe(rc.id);
    expect(previewManager(sound, agents)).toBeUndefined();
    expect(previewManager({ ...sound, name: "X Desk" }, agents)).toBeUndefined();
    expect(previewManager(desk, [desk, sound])).toBeUndefined();
    const scene = createScene(source, false);
    expect(scene.regions.map(region => region.members.map(agent => agent.id))).toEqual([[rc.id, desk.id], [sound.id]]);
  });
  it("derives plugin membership from saved capability selections, including partial grants", () => {
    const scene = createScene(source, false);
    expect(pluginMembers(scene.agents, "builtin").map(agent => agent.id)).toEqual([rc.id, desk.id]);
    expect(pluginMembers(scene.agents, "mail").map(agent => agent.id)).toEqual([sound.id]);
    const revoked = createScene(source.map(agent => ({ ...agent, capabilityIds: [] })), false);
    expect(pluginMembers(revoked.agents, "builtin")).toEqual([]);
    expect(pluginMembers(scene.agents, "unknown")).toEqual([]);
  });
  it("keeps stress-scene identities separate from Host records and fits 50 unique agents", () => {
    const scene = createScene(source, true);
    expect(scene.agents).toHaveLength(50);
    expect(new Set(scene.agents.map(agent => agent.id)).size).toBe(50);
    expect(scene.agents.filter(agent => agent.synthetic)).toHaveLength(47);
    expect(scene.agents.filter(agent => agent.synthetic).every(agent => !agent.summary)).toBe(true);
    expect(scene.regions.flatMap(region => region.members)).toHaveLength(50);
    expect(source).toHaveLength(3);
    expect(createScene(source, false).agents).toHaveLength(3);
  });
  it.each([
    ["quiet", "idle"], ["pending", "running"], ["interrupted", "failed"], ["waiting", "needs-you"],
  ] as const)("reports a %s member's state without inventing a more urgent member", (tone, state) => {
    const scene = createScene(source.map(agent => ({ ...agent, summary: summary(tone) })), false);
    expect(scene.regions.map(region => region.state)).toEqual([state, state]);
  });
  it("reports the most urgent member actually present in a mixed region", () => {
    const scene = createScene([
      { ...source[0]!, summary: summary("pending") },
      { ...source[1]!, summary: summary("interrupted") },
      source[2]!,
    ], false);
    expect(scene.regions.map(region => region.state)).toEqual(["failed", "idle"]);
    expect(scene.regions[0]!.counts).toEqual({ "needs-you": 0, failed: 1, running: 1, idle: 0 });
  });
  it("rearranges the phone scene without changing membership or access", () => {
    const desktop = createScene(source, false);
    const phone = createScene(source, false, 2);
    expect(phone.agents.map(agent => agent.id)).toEqual(desktop.agents.map(agent => agent.id));
    expect(phone.agents[2]!.position.y).toBeGreaterThan(phone.agents[1]!.position.y);
    expect(pluginMembers(phone.agents, "builtin").map(agent => agent.id)).toEqual([rc.id, desk.id]);
    const managedContentBottom = Math.max(...phone.regions[0]!.members.map(agent => agent.position.y)) + 115;
    const independentLabelTop = phone.regions[1]!.bounds.y + phone.regions[1]!.bounds.height * .04;
    expect(independentLabelTop - managedContentBottom).toBeGreaterThanOrEqual(32);
  });
  it("shares one day axis across every agent, so their strips are comparable", () => {
    const first = 1_000_000, second = 4_000_000;
    const marked: SpaceAgent[] = [
      { ...source[0]!, summary: { day: [{ at: first, state: "idle" }] } as never },
      { ...source[1]!, summary: { day: [{ at: second, state: "needs-you" }] } as never },
    ];
    const scene = createScene(marked, false);
    // One axis spanning the whole scene, not one per agent, so an early mark and
    // a late mark land at opposite ends of every strip.
    expect(scene.axis).toEqual({ start: first - 60_000, end: second + 60_000 });
    expect(createScene(source, false).axis).toBeNull();
    expect(createScene([], false)).toEqual({ agents: [], regions: [], axis: null });
  });
  it("handles empty scenes and produces finite closed contours for every crowded group", () => {
    expect(regionPath([])).toBe("");
    for (const region of createScene(source, true).regions) {
      const path = regionPath(region.members.map(agent => agent.position));
      expect(path).toMatch(/^M.+ Z$/);
      expect(path).not.toMatch(/NaN|Infinity/);
    }
  });
});
