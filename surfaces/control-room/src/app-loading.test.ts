import { describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { App } from "./App";

// The Host panel is a lazy chunk. Holding it pending mimics a slow first load,
// where the route must still show its loading state instead of an empty page.
vi.mock("./host-panel", () => ({
  HostPanel: () => { throw new Promise(() => undefined); },
}));

describe("the Host route while its panel chunk is pending", () => {
  it("keeps a static loading state on screen instead of rendering nothing", () => {
    vi.stubGlobal("window", { location: { search: "?host", hash: "" } });
    const html = renderToStaticMarkup(createElement(App));
    expect(html).toContain("Opening your Host");
    expect(html).toContain("host-loading");
  });
});
