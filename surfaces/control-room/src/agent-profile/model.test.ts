import { describe, expect, it } from "vitest";
import { hasProfileChanges, initialProfile, mergeProfileScope, nextSchedule } from "./model";

describe("independent editor drafts", () => {
  it("treats toggling a capability off and back on as unchanged", () => {
    const saved = initialProfile();
    expect(hasProfileChanges(saved, { ...saved, capabilities: [...saved.capabilities].reverse() }, "agent")).toBe(false);
  });
  it("keeps a newly saved schedule when an older agent draft is saved", () => {
    const saved = { ...initialProfile(), briefTime: "20:00", inboxEnabled: false };
    const olderDraft = { ...initialProfile(), name: "Research assistant", maxTokens: 4096, capabilities: ["web"] };
    expect(mergeProfileScope(saved, olderDraft, "agent")).toEqual({ ...saved, name: "Research assistant", maxTokens: 4096, capabilities: ["web"] });
  });
  it("never admits unsaved agent settings through a schedule save", () => {
    const saved = initialProfile();
    const draft = { ...saved, name: "Unfinished name", capabilities: [], briefEnabled: true };
    expect(mergeProfileScope(saved, draft, "brief")).toEqual({ ...saved, briefEnabled: true });
    expect(hasProfileChanges(saved, draft, "inbox")).toBe(false);
    expect(hasProfileChanges(saved, draft, "agent")).toBe(true);
  });
});

describe("schedule overview at the labelled 09:42 example time", () => {
  it("keeps an elapsed daily schedule behind today's inbox check", () => {
    expect(nextSchedule({ ...initialProfile(), briefEnabled: true, briefTime: "08:00" }))
      .toEqual({ part: "inbox", time: "10:00 IST" });
  });
  it("shows tomorrow when only an elapsed daily schedule is enabled", () => {
    expect(nextSchedule({ ...initialProfile(), inboxEnabled: false, briefEnabled: true, briefTime: "08:00" }))
      .toEqual({ part: "brief", time: "Tomorrow, 08:00 IST" });
  });
  it("selects a daily run that is still ahead and earlier than the inbox check", () => {
    expect(nextSchedule({ ...initialProfile(), briefEnabled: true, briefTime: "09:50" }))
      .toEqual({ part: "brief", time: "09:50 IST" });
  });
  it("never presents a paused schedule as upcoming", () => {
    expect(nextSchedule({ ...initialProfile(), inboxEnabled: false })).toBeNull();
    expect(nextSchedule({ ...initialProfile(), briefTime: "09:50" }))
      .toEqual({ part: "inbox", time: "10:00 IST" });
  });
});
