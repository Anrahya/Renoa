import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import type { Controls } from "./host-controls";
import { DiscordSetup } from "./host-discord-setup";
import { isBinding, parseChannels, parseDiscord, pendingOperation, readOwner, writeOwner, type DiscordChannel, type DiscordState } from "./agent-onboarding-client";

export function DiscordConnection({ agentId, controls }: { agentId: string; controls: Controls }) {
  const path = `/v1/host/discord/bindings?agent=${encodeURIComponent(agentId)}`;
  const [state, setState] = useState<DiscordState | null>(null);
  const [channels, setChannels] = useState<DiscordChannel[] | null>(null);
  const [channel, setChannel] = useState("");
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (controls.preview) return;
    const controller = new AbortController();
    const report = (error: unknown) => { if (!controller.signal.aborted) setNotice(error instanceof Error ? error.message : String(error)); };
    readOwner("/v1/host/discord", parseDiscord, controller.signal).then(async value => {
      setState(value);
      if (value.status === "connected") setChannels(await readOwner("/v1/host/discord/channels", parseChannels, controller.signal));
    }).catch(report);
    try { const saved = pendingOperation(controls.hostId, path); setPending(saved !== null); if (saved) setChannel(String(saved.channel_id)); }
    catch (error) { report(error); }
    return () => controller.abort();
  }, [controls.hostId, controls.preview, attempt, path]);
  function connected(value: DiscordState) { setState(value); setNotice("Discord connected. Choose a channel for this agent."); setAttempt(value => value + 1); controls.refresh(); }
  async function bind(event: React.FormEvent) {
    event.preventDefault(); if (busy || controls.preview || state?.status !== "connected") return;
    setBusy(true); setNotice(null);
    try {
      const existing = state.bindings.find(binding => binding.channel_id === channel);
      const result = await writeOwner(controls.hostId, path, { channel_id: channel, agent_id: agentId, expected_revision: existing?.revision ?? 0 }, (value, request) => isBinding(value) && value.agent_id === request.agent_id && value.channel_id === request.channel_id);
      setPending(result.kind === "uncertain");
      if (result.kind === "saved") { setNotice("Channel binding saved. Messages in this channel route to this agent."); setAttempt(value => value + 1); controls.refresh(); }
      else setNotice(result.message);
    } catch (error) { setNotice(error instanceof Error ? error.message : "Could not preserve the channel binding request."); }
    finally { setBusy(false); }
  }
  const options = channels && channel && !channels.some(item => item.id === channel) ? [...channels, { id: channel, name: channel }] : channels ?? [];
  return <section className="profile-settings-section" aria-labelledby="discord-connection-title"><div><h3 id="discord-connection-title" className="font-medium">Discord</h3><p className="mt-1 text-muted-foreground">Talk to this agent in your server</p></div><div className="flex min-w-0 flex-col gap-4">
    {controls.preview ? <p className="text-sm text-muted-foreground">Discord connections are available on your live Host.</p>
    : state?.status === "setup_required" ? <DiscordSetup agentId={agentId} controls={controls} onConnected={connected} onUncertain={() => setAttempt(value => value + 1)} />
    : state ? <>
      <p className="text-sm">Connected as <span className="font-medium">{state.bot_name}</span> in <span className="font-medium">{state.guild_name}</span>.{state.default_agent_id === agentId && " This agent also answers direct messages and mentions outside routed channels."}</p>
      <ul className="flex flex-col gap-2 text-sm" aria-label="Saved Discord channel bindings">{state.bindings.filter(binding => binding.agent_id === agentId).map(binding => <li key={binding.channel_id}>#{binding.channel_name}</li>)}</ul>
      <form onSubmit={event => void bind(event)} className="flex flex-col gap-3">
        <label className="flex flex-col gap-2 text-sm">Channel<NativeSelect required value={channel} disabled={busy || pending || channels === null} onChange={event => setChannel(event.target.value)}>
          <NativeSelectOption value="" disabled>{channels === null ? "Loading channels…" : "Choose a channel"}</NativeSelectOption>
          {options.map(item => <NativeSelectOption key={item.id} value={item.id}>#{item.name}</NativeSelectOption>)}
        </NativeSelect></label>
        {state.bindings.some(binding => binding.channel_id === channel && binding.agent_id !== agentId) && <p className="text-sm">This channel is assigned to another agent. Saving will route new messages to this agent; earlier work keeps its original target.</p>}
        <Button className="w-fit" disabled={busy || !controls.available || !channel}>{busy ? "Saving…" : pending ? "Retry saved binding" : "Connect channel"}</Button>
      </form>
      <p className="text-xs text-muted-foreground">Saved bindings do not prove current bot connectivity. Plugin approval links go privately to the application owner.</p>
    </> : <p className="text-sm text-muted-foreground">Loading Discord settings…</p>}
    {notice && <p role="status" className="text-sm">{notice}</p>}
    {!controls.preview && <Button variant="ghost" className="w-fit" disabled={busy} onClick={() => setAttempt(value => value + 1)}>Refresh Discord settings</Button>}
  </div></section>;
}
