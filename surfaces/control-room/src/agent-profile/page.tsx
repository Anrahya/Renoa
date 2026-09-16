import { useRef, useState } from "react";
import { CaretRight, ArrowCounterClockwise } from "@phosphor-icons/react";
import { ProfileDrawer } from "./drawer";
import { ProfileConnections } from "./connections";
import { capabilitySummary, hasProfileChanges, initialProfile, mergeProfileScope, nextSchedule, partTitles, profileScope, selectedCapabilities, type ProfileDraft, type ProfilePart, type ProfileScope } from "./model";
import "../styles/host.css";
import "./profile.css";
import "./editor.css";

export default function AgentProfilePreview() {
  const [profile, setProfile] = useState(initialProfile);
  const [part, setPart] = useState<ProfilePart | null>(null);
  const [drafts, setDrafts] = useState<Partial<Record<ProfileScope, ProfileDraft>>>({});
  const [lastEdited, setLastEdited] = useState<ProfilePart>("setup");
  const [notice, setNotice] = useState("");
  const [changed, setChanged] = useState(false);
  const stage = useRef<HTMLDivElement>(null);
  const skills = selectedCapabilities(profile, "skill").map(item => item.name).join(", ");
  const next = nextSchedule(profile);
  const nextIsInbox = next?.part === "inbox";
  const hasNext = next !== null;
  const open = (value: ProfilePart) => setPart(value);
  const scope = part ? profileScope(part) : null;
  const draft = scope ? mergeProfileScope(profile, drafts[scope] ?? profile, scope) : profile;
  const dirty = scope ? hasProfileChanges(profile, draft, scope) : false;
  const pendingScopes = Object.entries(drafts).filter(([key, value]) => hasProfileChanges(profile, value, key as ProfileScope)).map(([key]) => key);
  const resumePart = pendingScopes.includes(profileScope(lastEdited) ?? "") ? lastEdited : pendingScopes[0] === "agent" ? "setup" : pendingScopes[0] as ProfilePart;
  const clearDraft = (value: ProfileScope) => setDrafts(current => {
    const next = { ...current }; delete next[value]; return next;
  });
  return <div className="ap-app">
    <a className="ap-skip" href="#agent-profile-main">Skip to agent profile</a>
    <header className="ap-header">
      <a className="ap-wordmark" href="/">renoa<span>.</span></a>
      <nav aria-label="Host navigation">
        <a href="/?preview#overview">System</a><a href="/?preview#agents" aria-current="page">Agents</a>
        <a href="/?preview#work">Work</a><a href="/?preview#library">Library</a>
      </nav>
      <span className="ap-preview-label"><span />Interactive preview <span className="ap-preview-scope">· Changes stay in this tab</span></span>
    </header>
    <main id="agent-profile-main">
      <div className="ap-mobile-breadcrumb"><a href="/?preview#agents">All agents</a><span>Example data · 16 Sep 2026, 09:42 IST</span></div>
      <div className="ap-stage" ref={stage}>
        <ProfileConnections stage={stage} />
        <section className="ap-identity" aria-label="Agent identity">
          <img data-profile-anchor className="ap-portrait" src="/assets/identities/arcee-prime.webp" alt="" width="190" height="190" />
          <h1>{profile.name}</h1><p>{profile.role}</p>
          <button className="ap-primary ap-customize" onClick={() => open("setup")}>Customize</button>
          <button className="ap-runtime-link" onClick={() => open("runtime")}>Renoa loop · VPS <CaretRight size={12} /></button>
          {pendingScopes.length > 0 && <button className="ap-resume-draft" onClick={() => open(resumePart)}>Resume unsaved changes</button>}
        </section>
        <div className="ap-part ap-instructions"><PartLink part="instructions" value="Purpose + 2 documents" onOpen={open} /></div>
        <div className="ap-part ap-model"><PartLink part="model" value={profile.model} extra={`${profile.maxTokens.toLocaleString()} output tokens`} onOpen={open} /></div>
        <div className="ap-part ap-tools">
          <PartLink part="tools" value={capabilitySummary(profile)} extra={skills || "No skills selected"} onOpen={open} />
          {profile.capabilities.includes("drive") && <button className="ap-connection-warning" onClick={() => open("tools")}>
            <span className="ap-status-dot" /><span>Drive needs reconnection<small>View connection</small></span>
          </button>}
        </div>
        <section className="ap-work" aria-label="Agent work overview">
          <button className="ap-work-item ap-current" onClick={() => open("current")}>
            <span className="ap-state ap-state-now">Now <span className="ap-example">Example</span></span>
            <span data-component-anchor="current" className="ap-current-title">Preparing morning brief</span>
            <span className="ap-work-detail">Reading sources · Started 09:40 IST</span>
          </button>
          <button className="ap-work-item ap-next" onClick={() => open(nextIsInbox || !hasNext ? "inbox" : "brief")}>
            <span className="ap-state">Next</span>
            <span className="ap-next-time">{next?.time ?? "No scheduled work"}</span>
            <span className="ap-work-title">{hasNext ? nextIsInbox ? "Inbox check" : "Evening brief" : "Both schedules are paused"}</span>
            <span className="ap-work-detail">{hasNext ? nextIsInbox ? "Every 30 min" : "Daily" : "Manage schedules"}<CaretRight size={13} /></span>
          </button>
          <button className="ap-work-item ap-scheduled" onClick={() => open(nextIsInbox || !hasNext ? "brief" : "inbox")}>
            <span className="ap-work-title">{nextIsInbox || !hasNext ? "Evening brief" : "Inbox check"}</span>
            <span className="ap-work-detail">{nextIsInbox || !hasNext ? `Daily ${profile.briefTime} IST · ${profile.briefEnabled ? "Enabled" : "Paused"}` : `Every 30 min · ${profile.inboxEnabled ? "Enabled" : "Paused"}`}<CaretRight size={13} /></span>
          </button>
          <button className="ap-work-item ap-recent" onClick={() => open("recent")}>
            <span className="ap-state ap-state-muted">Last completed</span>
            <span className="ap-work-title">Brief delivered</span>
            <span className="ap-work-detail">Yesterday, 19:00 · 6 items summarized<CaretRight size={13} /></span>
          </button>
        </section>
      </div>
      <footer className="ap-footnote"><span>Example state · 16 September 2026, 09:42 IST</span>
        {changed ? <button className="ap-text-button" onClick={() => { setProfile(initialProfile()); setDrafts({}); setChanged(false); setNotice("Preview reset. Original example restored."); }}><ArrowCounterClockwise size={14} /> Reset preview</button> : <span>Select a part to inspect or customize it.</span>}
      </footer>
      <p className="ap-announcement" role="status" aria-live="polite">{notice}</p>
    </main>
    {part && <ProfileDrawer key={scope ?? part} part={part} profile={profile} draft={draft} dirty={dirty}
      onNavigate={open} onClose={() => setPart(null)} onChange={next => {
        if (scope) { setDrafts(current => ({ ...current, [scope]: next })); setLastEdited(part); }
      }} onDiscard={() => { if (scope) clearDraft(scope); setPart(null); setNotice("Draft discarded."); }}
      onSave={() => {
        if (!scope) return;
        setProfile(mergeProfileScope(profile, draft, scope)); clearDraft(scope); setChanged(true); setPart(null);
        setNotice(`${scope === "agent" ? "Agent setup" : partTitles[part]} saved in this preview.`);
      }} />}

  </div>;
}

function PartLink({ part, value, extra, onOpen }: { part: ProfilePart; value: string; extra?: string; onOpen: (part: ProfilePart) => void }) {
  return <button className="ap-part-link" onClick={() => onOpen(part)} aria-label={`Open ${partTitles[part]}`}>
    <span className="ap-part-label" data-component-anchor={part}>{partTitles[part]}<CaretRight size={14} /></span>
    <span className="ap-part-value">{value}</span>{extra && <span className="ap-part-value">{extra}</span>}
  </button>;
}
