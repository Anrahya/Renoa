import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import type { Controls } from "./host-controls";
import { isDiscordState, parseInspection, readOwner, submitOwner, type DiscordInspection, type DiscordState } from "./agent-onboarding-client";

/** Connects the Host's one Discord bot. The token lives only in this component until the Host stores it. */
export function DiscordSetup({ agentId, controls, onConnected, onUncertain }: { agentId: string; controls: Controls; onConnected: (state: DiscordState) => void; onUncertain: () => void }) {
  const [token, setToken] = useState("");
  const [inspection, setInspection] = useState<DiscordInspection | null>(null);
  const [guild, setGuild] = useState("");
  const [operation, setOperation] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const eligible = inspection?.guilds.filter(server => server.administrator) ?? [];
  function editToken(value: string) { setToken(value); setInspection(null); setGuild(""); setOperation(null); }
  function chooseGuild(value: string) { setGuild(value); setOperation(null); }
  async function check(event: React.FormEvent) {
    event.preventDefault(); if (busy || !controls.available) return;
    setBusy(true); setNotice(null);
    try {
      const result = await readOwner("/v1/host/discord/inspection", parseInspection, AbortSignal.timeout(30_000), { bot_token: token.trim() });
      setInspection(result);
      setGuild(current => result.guilds.some(server => server.id === current && server.administrator) ? current : result.guilds.find(server => server.administrator)?.id ?? "");
    } catch (error) { setNotice(error instanceof Error ? error.message : "The token could not be checked."); }
    finally { setBusy(false); }
  }
  async function connect() {
    if (busy || !guild || !controls.available) return;
    const operation_id = operation ?? crypto.randomUUID();
    setOperation(operation_id); setBusy(true); setNotice(null);
    try {
      const result = await submitOwner("/v1/host/discord/connection", { operation_id, bot_token: token.trim(), guild_id: guild, agent_id: agentId },
        value => isDiscordState(value) && value.status === "connected" && value.default_agent_id === agentId);
      if (result.kind === "saved") { setToken(""); onConnected(result.record as unknown as DiscordState); return; }
      if (result.kind === "rejected") { setNotice(result.message); return; }
      setNotice("The connection could not be confirmed. Connect again to recover it; this page still holds the token.");
      onUncertain();
    } finally { setBusy(false); }
  }
  return <div className="flex flex-col gap-4">
    <p className="text-sm">Connect a Discord bot to talk to your agents from your server.</p>
    <ol className="flex list-decimal flex-col gap-1 pl-5 text-sm text-muted-foreground">
      <li>In the Developer Portal, create an application and open its Bot page.</li>
      <li>Turn on Message Content Intent, then reset and copy the bot token.</li>
      <li>Paste it below. The Host keeps it in a private file; this browser does not store it.</li>
    </ol>
    <Button variant="outline" className="w-fit" asChild><a href="https://discord.com/developers/applications" target="_blank" rel="noreferrer">Open Discord Developer Portal</a></Button>
    <form onSubmit={event => void check(event)} className="flex flex-col gap-3">
      <label className="flex flex-col gap-2 text-sm">Bot token<Input type="password" autoComplete="off" spellCheck={false} required value={token} disabled={busy} onChange={event => editToken(event.target.value)} /></label>
      <Button className="w-fit" variant={inspection ? "outline" : "default"} disabled={busy || !controls.available || !token.trim()}>{busy && !inspection ? "Checking…" : inspection ? "Check again" : "Check token"}</Button>
    </form>
    {inspection && <div className="flex flex-col gap-3">
      <p className="text-sm">Bot <span className="font-medium">{inspection.bot_name}</span> {inspection.guilds.length === 0 ? "is not in a server yet." : `is in ${inspection.guilds.length === 1 ? "1 server" : `${inspection.guilds.length} servers`}.`}</p>
      {inspection.guilds.length > 0 && <label className="flex flex-col gap-2 text-sm">Server<NativeSelect value={guild} disabled={busy} onChange={event => chooseGuild(event.target.value)}>
        <NativeSelectOption value="" disabled>Choose a server</NativeSelectOption>
        {inspection.guilds.map(server => <NativeSelectOption key={server.id} value={server.id} disabled={!server.administrator}>{server.administrator ? server.name : `${server.name} (needs Administrator)`}</NativeSelectOption>)}
      </NativeSelect></label>}
      {eligible.length === 0 && <p className="text-sm">Invite the bot with Administrator, then check again.</p>}
      <div className="flex flex-wrap gap-2">
        <Button className="w-fit" disabled={busy || !guild || !controls.available} onClick={() => void connect()}>{busy ? "Connecting…" : "Connect Discord"}</Button>
        <Button variant="outline" className="w-fit" asChild><a href={inspection.invite_url} target="_blank" rel="noreferrer">Invite with Administrator</a></Button>
      </div>
      <p className="text-xs text-muted-foreground">One bot serves this Host and cannot be changed here after connecting. This agent answers direct messages and mentions outside routed channels; plugin approval links go privately to the application owner.</p>
    </div>}
    {notice && <p role="status" className="text-sm">{notice}</p>}
  </div>;
}
