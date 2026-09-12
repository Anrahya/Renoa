export interface HostEntry {
  readonly href: string;
  readonly label: "Preview Host" | "Open Host";
}

export function homepageHostEntry(development: boolean): HostEntry {
  return development
    ? { href: "/?preview#overview", label: "Preview Host" }
    : { href: "/?host", label: "Open Host" };
}

export function liveHostHref(development: boolean): string {
  return development ? "https://renoa.live/?host" : "/?host";
}
