import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowClockwise, SignOut } from "@phosphor-icons/react";
import { useHost } from "./use-host";
import { HostLogin } from "./host-login";
import { AgentsView } from "./host-agents";
import { ConnectionsView } from "./host-library";
import { WorkView } from "./host-work";
import { SystemView } from "./host-system";
import { liveHostHref } from "./host-entry";
import { hostRoute } from "./host-navigation";
import { timestamp } from "./host-presentation";
import "./styles/host.css";

export function HostPanel() {
  const host = useHost();
  return <HostPanelView host={host} />;
}
export function HostPanelView({ host, preview = false, previewLabel = "Example data", previewAction, demo = false }: {
  host: ReturnType<typeof useHost>; preview?: boolean; previewLabel?: string; previewAction?: ReactNode; demo?: boolean;
}) {
  const [hash, setHash] = useState(() => window.location.hash);
  const route = hostRoute(hash);
  const previous = useRef(hash);
  const [logoutError, setLogoutError] = useState<string | null>(null);
  useEffect(() => {
    const update = () => setHash(window.location.hash);
    window.addEventListener("hashchange", update);
    return () => window.removeEventListener("hashchange", update);
  }, []);
  useEffect(() => {
    if (previous.current === hash) return;
    const before = hostRoute(previous.current); previous.current = hash;
    // Keep section-navigation focus in place; focus the title on page changes.
    if (before.view === route.view && before.agent === route.agent) return;
    const heading = document.querySelector<HTMLElement>(".host-content h1");
    if (heading) { heading.tabIndex = -1; heading.focus({ preventScroll: true }); }
    window.scrollTo({ top: 0 });
  }, [hash, route.agent, route.view]);
  async function logout() {
    try {
      const response = await fetch("/v1/identity/logout", { method: "POST", credentials: "same-origin", cache: "no-store" });
      if (!response.ok) throw new Error("Sign-out did not complete. Please retry when the Host is available.");
      setLogoutError(null); host.lock();
    } catch (error) { setLogoutError(error instanceof Error ? error.message : "Could not sign out."); }
  }
  const signedIn = host.snapshot !== null;
  const controls = { hostId: host.snapshot?.host_id ?? "", refresh: host.refresh, available: host.status === "connected", preview };
  return <div className="host-app"><a className="host-skip" href="#host-main" onClick={event => {
    event.preventDefault();
    const main = document.getElementById("host-main");
    if (main) { main.tabIndex = -1; main.focus(); }
  }}>Skip to content</a><header className="host-header">
    <a className="host-brand" href="/" aria-label="Renoa home">renoa<span>.</span></a>
    {signedIn && <nav aria-label="Host navigation">{(["overview", "agents", "work", "library"] as const).map(item =>
      <a key={item} href={`#${item}`} aria-current={route.view === item ? "page" : undefined}>{item === "library" ? "Library" : item === "overview" ? "System" : item === "work" ? "Work" : "Agents"}</a>)}</nav>}
    <div className="host-header-status"><span className={host.status === "connected" && !preview ? "host-online" : "host-secondary"}>
      {preview ? "Read-only preview" : host.status === "connected" ? "Host connected" : host.status === "reconnecting" ? "Reconnecting" : host.status === "connecting" ? "Connecting" : "Private Host"}</span>
      {signedIn && !preview && <><button aria-label="Refresh Host" title="Refresh Host" className="host-icon" onClick={host.refresh}><ArrowClockwise size={18} /></button>
        <button aria-label="Sign out of this browser" title="Sign out of this browser" className="host-icon" onClick={() => void logout()}><SignOut size={18} /></button></>}
    </div></header>
    {preview && <div className="host-banner"><span>{previewLabel}{host.receivedAt && !demo && ` · ${timestamp(host.receivedAt)}`}</span>{previewAction}<a className="host-link" href={liveHostHref(import.meta.env.DEV)}>Open live Host</a></div>}
    {(host.error || logoutError) && host.status !== "forbidden" && <div className="host-banner host-banner-error" role="status">
      <span>{logoutError ?? host.error}{host.receivedAt && ` Showing records from ${timestamp(host.receivedAt)}.`}</span>
      <button className="host-link" onClick={host.refresh}>Retry now</button></div>}
    {host.status === "locked" || host.status === "forbidden" ? <HostLogin refresh={host.refresh} forbidden={host.status === "forbidden"} />
      : host.snapshot ? <>
        {route.view === "overview" && <SystemView host={host.snapshot} live={(!preview || demo) && host.status === "connected"} receivedAt={host.receivedAt} />}
        {route.view === "work" && <WorkView host={host.snapshot} controls={controls} />}
        {route.view === "agents" && <AgentsView host={host.snapshot} route={route} controls={controls} />}
        {route.view === "library" && <ConnectionsView host={host.snapshot} />}
      </> : <main id="host-main" className="host-content host-loading" aria-busy="true">
        <h1>{host.status === "reconnecting" ? "Waiting for your Host" : "Opening your Host"}</h1>
        <p className="host-intro">{host.status === "reconnecting" ? "We’ll reconnect when it returns. Your login stays in this browser." : "Restoring your browser’s remembered session."}</p></main>}
    {host.snapshot && <footer className="host-footer"><details><summary>Host identity</summary><code>{host.snapshot.host_id}</code></details>
      <span>{demo ? "Example activity" : host.receivedAt && `${preview ? "Snapshot saved" : "Last received"} ${timestamp(host.receivedAt)}`}</span></footer>}
  </div>;
}
