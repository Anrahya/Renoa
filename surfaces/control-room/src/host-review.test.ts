import { describe, expect, it } from "vitest";
import { parseReviewDetail } from "./host-review";

describe("review diagnostics", () => {
  const detail = { request_id: "review", provider: "provider", model: "model", reasoning: "high", reason: "Provider returned no output", report: null };
  it("reads incomplete outcomes without requiring a report", () => {
    expect(parseReviewDetail(detail, "review").reason).toBe("Provider returned no output");
  });
  it("rejects mismatched identities and malformed findings rather than inventing a clean review", () => {
    expect(() => parseReviewDetail(detail, "another review")).toThrow();
    expect(() => parseReviewDetail({ ...detail, report: { findings: [{}], limitations: [] } }, "review")).toThrow();
    expect(parseReviewDetail({ ...detail, reason: null, report: { findings: [], limitations: ["Tests were not run"] } }, "review").report?.limitations).toEqual(["Tests were not run"]);
  });
});
