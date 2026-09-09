import { useEffect, useState } from "react";
import { ArrowClockwise, SignOut } from "@phosphor-icons/react";
import { useHost } from "./use-host";
import { HostLogin } from "./host-login";
import { AgentsView, ConnectionsView } from "./host-agents";
import { WorkView } from "./host-work";
import "./styles/host.css";

type View = "work" | "agents" | "connections";
export function HostPanel() {
  const host = useHost();
  return <HostPanelView host={host} />;
}
export function HostPanelView({ host, preview = false }: { host: ReturnType<typeof useHost>; preview?: boolean }) {
  const [view, setView] = useState<View>("work");
  const [selected, setSelected] = useState<string | null>(null);
  const [logoutError, setLogoutError] = useState<string | null>(null);
  useEffect(() => {
    const heading = document.querySelector<HTMLElement>(".host-content h1");
    if (heading) { heading.tabIndex = -1; heading.focus({ preventScroll: true }); }
  }, [view, selected]);
  function navigate(next: View) { setView(next); setSelected(null); window.scrollTo({ top: 0 }); }
  function openAgent(id: string | null) { setView("agents"); setSelected(id); window.scrollTo({ top: 0 }); }
  async function logout() {
    try {
      const response = await fetch("/v1/identity/logout", { method: "POST", credentials: "same-origin", cache: "no-store" });
      if (!response.ok) throw new Error("Sign-out did not complete. Please retry when the Host is available.");
      setLogoutError(null); host.lock();
    } catch (error) { setLogoutError(error instanceof Error ? error.message : "Could not sign out."); }
  }
  const signedIn = host.snapshot !== null;
  const controls = { hostId: host.snapshot?.host_id ?? "", refresh: host.refresh, available: host.status === "connected", preview };
  return <div className="host-app"><header className="host-header">
    <a className="host-brand" href="/" onClick={event => { event.preventDefault(); navigate("work"); }}>renoa</a>
    {signedIn && <nav aria-label="Host navigation">{(["work", "agents", "connections"] as const).map(item =>
      <button key={item} aria-current={view === item ? "page" : undefined} onClick={() => navigate(item)}>
        {item === "connections" ? "Shared library" : item.charAt(0).toUpperCase() + item.slice(1)}</button>)}</nav>}
    <div className="host-header-status"><span>{signedIn ? "Personal Host" : "Your personal system"}</span>
      <span className={host.status === "connected" ? "host-online" : "host-secondary"}>
        {preview ? "Example data" : host.status === "connected" ? "Connected" : host.status === "reconnecting" ? "Reconnecting" : host.status === "connecting" ? "Connecting" : "Private"}</span>
      {signedIn && !preview && <><button aria-label="Refresh Host" className="host-icon" onClick={host.refresh}><ArrowClockwise size={18} /></button>
        <button aria-label="Sign out of this browser" className="host-icon" onClick={() => void logout()}><SignOut size={18} /></button></>}
    </div></header>
    {preview && <div className="host-banner">Design preview · Example data <a className="host-link" href="/">Open your live Host</a></div>}
    {(host.error || logoutError) && host.status !== "forbidden" && <div className="host-banner" role="status">
      <span>{logoutError ?? host.error}{host.receivedAt && ` Last received ${new Date(host.receivedAt).toLocaleTimeString()}.`}</span>
      <button className="host-link" onClick={host.refresh}>Retry now</button></div>}
    {host.status === "locked" || host.status === "forbidden" ? <HostLogin refresh={host.refresh} forbidden={host.status === "forbidden"} />
      : host.snapshot ? <>
        {view === "work" && <WorkView host={host.snapshot} openAgent={openAgent} controls={controls} />}
        {view === "agents" && <AgentsView host={host.snapshot} selected={selected} openAgent={openAgent} connections={() => navigate("connections")} controls={controls} />}
        {view === "connections" && <ConnectionsView host={host.snapshot} openAgent={openAgent} />}
      </> : <main className="host-content host-loading"><p className="host-kicker">Your system, within reach</p>
        <h1>{host.status === "reconnecting" ? "Waiting for your Host." : "Opening your Host."}</h1>
        <p className="host-intro">{host.status === "reconnecting" ? "We’ll reconnect when it returns. Your login stays in this browser." : "Restoring your browser’s remembered session."}</p></main>}
    {host.snapshot && <footer className="host-footer"><details><summary>Host identity</summary><code>{host.snapshot.host_id}</code></details>
      <span>{host.receivedAt && `Last received ${new Date(host.receivedAt).toLocaleTimeString()}`}</span></footer>}
  </div>;
}
