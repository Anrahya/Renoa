export function workDesignPreview(preview: boolean): boolean {
  return import.meta.env.DEV && preview && new URLSearchParams(window.location.search).has("preview");
}
