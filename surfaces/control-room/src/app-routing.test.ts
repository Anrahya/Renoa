import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { App } from "./App";
import { useHost } from "./use-host";
import { useControlRoom } from "./use-control-room";

vi.mock("./use-host", { spy: true });
vi.mock("./use-control-room", { spy: true });

afterEach(() => { vi.clearAllMocks(); vi.unstubAllGlobals(); });

describe("the public homepage and private surfaces", () => {
  it("renders the product introduction without mounting either private controller", () => {
    vi.stubGlobal("window", { location: { search: "" } });
    const html = renderToStaticMarkup(createElement(App));
    expect(html).toContain("A system<br/>of your");
    expect(html).toContain('href="/?host"');
    expect(useHost).not.toHaveBeenCalled();
    expect(useControlRoom).not.toHaveBeenCalled();
  });

  it("opens the existing Host surface through the homepage's query route", () => {
    vi.stubGlobal("window", { location: { search: "?host", hash: "" } });
    const html = renderToStaticMarkup(createElement(App));
    expect(useHost).toHaveBeenCalledOnce();
    expect(useControlRoom).not.toHaveBeenCalled();
    expect(html).toContain("Opening your Host");
    expect(html).not.toContain("renoa-form-canvas");
  });

  it("keeps the legacy task surface independently addressable", () => {
    vi.stubGlobal("window", { location: { search: "?tasks" } });
    vi.stubGlobal("localStorage", { getItem: () => null });
    const html = renderToStaticMarkup(createElement(App));
    expect(useControlRoom).toHaveBeenCalledOnce();
    expect(useHost).not.toHaveBeenCalled();
    expect(html).toContain("Create your passkey");
    expect(html).not.toContain("renoa-form-canvas");
  });
});
