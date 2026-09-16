import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { PreviewConnectionProvider } from "./connection-state";
import { ConfigureCapabilities } from "./configure-capabilities";
import { initialConfiguration } from "./configuration-model";

it("keeps plugin disclosure IDs distinct from their capability label targets", () => {
  const html = renderToStaticMarkup(<PreviewConnectionProvider><form><ConfigureCapabilities selected={initialConfiguration("Test agent").capabilities} change={() => {}} /></form></PreviewConnectionProvider>);
  // The Research plugin and its Research skill deliberately share a fixture ID.
  const ids = [...html.matchAll(/\sid="([^"]+)"/g)].map(match => match[1]);
  expect(new Set(ids).size).toBe(ids.length);
  const label = html.match(/for="([^"]+)"[^>]*>Research<\/label>/);
  expect(label).not.toBeNull();
  expect(html).toContain(`id="${label![1]}"`);
  expect(label![1]).toContain("-capability-research");
  expect(html).toContain("HTTP API");
  expect(html).toContain("Skills");
});
