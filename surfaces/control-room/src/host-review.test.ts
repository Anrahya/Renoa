import { describe, expect, it } from "vitest";
import { parseReviewDetail } from "./host-review";

describe("review diagnostics", () => {
  const detail = { request_id: "review", provider: "provider", model: "model", reasoning: "high", reason: "Provider returned no output", report: null,
    execution: null, publication: { state: "suppressed", reason: "Review incomplete" },
    repository: { revision: 1, policy: { repository_id: 1, installation_id: 1, full_name: "owner/repo", agent_id: "00000000-0000-0000-0000-000000000001", enabled: true, skip_drafts: true, triggers: ["opened"] } } };
  it("reads incomplete outcomes without requiring a report", () => {
    expect(parseReviewDetail(detail, "review").reason).toBe("Provider returned no output");
  });
  it("rejects mismatched identities and malformed findings rather than inventing a clean review", () => {
    expect(() => parseReviewDetail(detail, "another review")).toThrow();
    expect(() => parseReviewDetail({ ...detail, report: { findings: [{}], limitations: [] } }, "review")).toThrow();
    expect(parseReviewDetail({ ...detail, reason: null, report: { findings: [], limitations: ["Tests were not run"] } }, "review").report?.limitations).toEqual(["Tests were not run"]);
  });
  it("keeps old reports readable and validates deletion and body-only locations", () => {
    const finding = { path: "removed.rs", line: 1, title: "Missing check", trigger: "Delete the check", consequence: "Access allowed", correction: "Restore the check", evidence: { path: "removed.rs", start_line: 1, quote: "check_owner();" } };
    const read = (f: unknown) => parseReviewDetail({ ...detail, report: { findings: [f], limitations: [] } }, "review");
    expect(read(finding).report?.findings).toHaveLength(1);
    expect(read({ ...finding, side: "base", in_diff: false, evidence: { ...finding.evidence, side: "base" } }).report?.findings[0]?.side).toBe("base");
    expect(() => read({ ...finding, side: "wrong" })).toThrow();
    expect(() => read({ ...finding, in_diff: "false" })).toThrow();
    expect(() => read({ ...finding, evidence: { ...finding.evidence, side: "wrong" } })).toThrow();
  });
});
