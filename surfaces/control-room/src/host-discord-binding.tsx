import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { Controls } from "./host-controls";
import { isBinding, parseDiscord, pendingOperation, readOwner, writeOwner, type DiscordState } from "./agent-onboarding-client";

export function DiscordConnection({ agentId, controls }: { agentId: string; controls: Controls }) {
  const path = `/v1/host/discord/bindings?agent=${encodeURIComponent(agentId)}`;
  const [state, setState] = useState<DiscordState | null>(null);
  const [channel, setChannel] = useState("");
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (controls.preview) return;
    const controller = new AbortController();
    readOwner("/v1/host/discord", parseDiscord, controller.signal).then(setState).catch(error => { if (!controller.signal.aborted) setNotice(error.message); });
    try { const saved = pendingOperation(controls.hostId, path); setPending(saved !== null); if (saved) setChannel(String(saved.channel_id)); }
    catch (error) { setNotice(String(error)); }
    return () => controller.abort();
  }, [controls.hostId, controls.preview, attempt, path]);
  async function bind(event: React.FormEvent) {
    event.preventDefault(); if (busy || controls.preview || state?.status !== "configured") return;
    setBusy(true); setNotice(null);
    try {
      const existing = state.bindings.find(binding => binding.channel_id === channel);
      const result = await writeOwner(controls.hostId, path, { channel_id: channel, agent_id: agentId, expected_revision: existing?.revision ?? 0 }, (value, request) => isBinding(value) && value.agent_id === request.agent_id && value.channel_id === request.channel_id);
      setPending(result.kind === "uncertain");
      if (result.kind === "saved") { setNotice("Channel binding saved. Messages in this channel route to the selected agent."); setAttempt(value => value + 1); controls.refresh(); }
      else setNotice(result.message);
    } catch (error) { setNotice(error instanceof Error ? error.message : "Could not preserve the channel binding request."); }
    finally { setBusy(false); }
  }
  return <section className="profile-settings-section" aria-labelledby="discord-connection-title"><div><h3 id="discord-connection-title" className="font-medium">Discord</h3><p className="mt-1 text-muted-foreground">Talk to this agent in your channel</p></div><div className="flex min-w-0 flex-col gap-4">
    {controls.preview ? <p className="text-sm text-muted-foreground">Discord connections are available on your live Host.</p> : state?.status === "setup_required" ? <><p className="text-sm">Discord application setup is required before connecting a channel.</p><p className="text-sm text-muted-foreground">Create a bot in the Discord Developer Portal, enable Message Content intent, and invite it to your server. Keep its token out of chat; the Host reads it from a private token file.</p><Button variant="outline" className="w-fit" asChild><a href="https://discord.com/developers/applications" target="_blank" rel="noreferrer">Open Discord Developer Portal</a></Button></> : state ? <>
      <ul className="flex flex-col gap-2 text-sm" aria-label="Saved Discord channel bindings">{state.bindings.filter(binding => binding.agent_id === agentId).map(binding => <li key={binding.channel_id}>#{binding.channel_name} <span className="text-muted-foreground">· {binding.channel_id}</span></li>)}</ul>
      <form onSubmit={event => void bind(event)} className="flex flex-col gap-3"><label className="flex flex-col gap-2 text-sm">Channel ID<Input required inputMode="numeric" pattern="[1-9][0-9]{0,19}" value={channel} disabled={busy || pending} onChange={event => setChannel(event.target.value)} /></label><p className="text-xs text-muted-foreground">In Discord, enable Developer Mode and use Copy Channel ID. Choose a text channel the bot can read and send messages in.</p>
      {state.bindings.some(binding => binding.channel_id === channel && binding.agent_id !== agentId) && <p className="text-sm">This channel is assigned to another agent. Saving will route new messages to this agent; earlier work keeps its original target.</p>}
      <Button className="w-fit" disabled={busy || !controls.available || !channel}>{busy ? "Saving…" : pending ? "Retry saved binding" : "Connect channel"}</Button></form><p className="text-xs text-muted-foreground">Saved bindings do not prove current bot connectivity. Plugin approval links go privately to the configured operator.</p>
    </> : <p className="text-sm text-muted-foreground">Loading Discord settings…</p>}
    {notice && <p role="status" className="text-sm">{notice}</p>}
    {!controls.preview && <Button variant="ghost" className="w-fit" disabled={busy} onClick={() => setAttempt(value => value + 1)}>Refresh Discord settings</Button>}
  </div></section>;
}
