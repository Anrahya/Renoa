export function liveHostHref(development: boolean): string {
  return development ? "https://renoa.live/?host" : "/?host";
}
