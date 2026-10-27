import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToReadableStream, renderToStaticMarkup } from "react-dom/server";
import { App } from "./App";
import { liveHostHref } from "./host-entry";
import { useHost } from "./use-host";
import { useControlRoom } from "./use-control-room";

vi.mock("./use-host", { spy: true });
vi.mock("./use-control-room", { spy: true });

afterEach(() => { vi.clearAllMocks(); vi.unstubAllGlobals(); });

describe("the query routes and private surfaces", () => {
  it("renders no private surface and mounts neither controller without a query route", () => {
    vi.stubGlobal("window", { location: { search: "" } });
    const html = renderToStaticMarkup(createElement(App));
    expect(html).toBe("");
    expect(useHost).not.toHaveBeenCalled();
    expect(useControlRoom).not.toHaveBeenCalled();
  });

  it("targets the live Host only outside development", () => {
    expect(liveHostHref(true)).toBe("https://renoa.live/?host");
    expect(liveHostHref(false)).toBe("/?host");
  });

  it("opens the existing Host surface through its query route", async () => {
    vi.stubGlobal("window", { location: { search: "?host", hash: "" } });
    const stream = await renderToReadableStream(createElement(App));
    await stream.allReady;
    const html = await new Response(stream).text();
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
