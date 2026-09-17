import { describe, expect, it } from "vitest";
import { capabilityPlugins, pluginCapabilities, changedSections, initialConfiguration, setCapabilities } from "./configuration-model";
describe("configuration preview drafts", () => {
  it("keeps the source library and other selections when a group is disabled", () => {
    const saved = initialConfiguration("Test agent");
    const mail = capabilityPlugins.find(plugin => plugin.id === "mail")!;
    const items = pluginCapabilities(mail);
    const withoutMail = setCapabilities(saved.capabilities, items.map(item => item.id), false);
    expect(withoutMail).not.toContain("mail-read");
    expect(withoutMail).not.toContain("mail-send");
    expect(withoutMail).toContain("web");
    expect(saved.capabilities).toContain("mail-read");
    expect(items).toHaveLength(2);
    expect(setCapabilities(withoutMail, items.map(item => item.id), true).sort()).toEqual([...saved.capabilities].sort());
  });
  it("selects a plugin's tools and skills together without changing other plugins", () => {
    const saved = initialConfiguration("Test agent");
    const research = capabilityPlugins.find(plugin => plugin.id === "research")!;
    const ids = pluginCapabilities(research).map(item => item.id);
    const disabled = setCapabilities(saved.capabilities, ids, false);
    expect(disabled).toEqual(["read", "search", "mail-read", "mail-send"]);
    expect(setCapabilities(disabled, ids, true)).toEqual([...disabled, "web", "research", "writing", "review"]);
  });
  it("reports only changed sections and ignores selection ordering", () => {
    const saved = initialConfiguration("Test agent");
    expect(changedSections(saved, { ...saved, capabilities: [...saved.capabilities].reverse() })).toEqual([]);
    expect(changedSections(saved, { ...saved, name: "Renamed", maxTokens: 4096 })).toEqual(["Model & identity"]);
    expect(changedSections(saved, { ...saved, behavior: "Be brief", preferences: "Use UTC" })).toEqual(["Instructions"]);
    expect(changedSections(saved, { ...saved, capabilities: [] })).toEqual(["Capabilities"]);
  });
});
