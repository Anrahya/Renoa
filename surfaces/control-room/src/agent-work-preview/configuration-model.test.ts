import { describe, expect, it } from "vitest";
import { capabilitySources, changedSections, initialConfiguration, setCapabilities } from "./configuration-model";
describe("configuration preview drafts", () => {
  it("keeps the source library and other selections when a group is disabled", () => {
    const saved = initialConfiguration("Test agent");
    const mail = capabilitySources.find(source => source.id === "mail")!;
    const withoutMail = setCapabilities(saved.capabilities, mail.items.map(item => item.id), false);
    expect(withoutMail).not.toContain("mail-read");
    expect(withoutMail).not.toContain("mail-send");
    expect(withoutMail).toContain("web");
    expect(saved.capabilities).toContain("mail-read");
    expect(mail.items).toHaveLength(2);
    expect(setCapabilities(withoutMail, mail.items.map(item => item.id), true).sort()).toEqual([...saved.capabilities].sort());
  });
  it("reports only changed sections and ignores selection ordering", () => {
    const saved = initialConfiguration("Test agent");
    expect(changedSections(saved, { ...saved, capabilities: [...saved.capabilities].reverse() })).toEqual([]);
    expect(changedSections(saved, { ...saved, name: "Renamed", maxTokens: 4096 })).toEqual(["Model & identity"]);
    expect(changedSections(saved, { ...saved, behavior: "Be brief", preferences: "Use UTC" })).toEqual(["Instructions"]);
    expect(changedSections(saved, { ...saved, capabilities: [] })).toEqual(["Capabilities"]);
  });
});
