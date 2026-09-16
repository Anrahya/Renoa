import type { ProfileDraft, ProfilePart } from "./model";
import { SetupEditor, ModelEditor, InstructionsEditor } from "./setup-editor";
import { CapabilitiesEditor } from "./capabilities-editor";
import "./editor.css";

type EditorProps = { part: ProfilePart; draft: ProfileDraft; onChange: (next: ProfileDraft) => void; onNavigate: (part: ProfilePart) => void };

function Schedule({ part, draft, onChange }: EditorProps) {
  const inbox = part === "inbox";
  const enabled = inbox ? draft.inboxEnabled : draft.briefEnabled;
  return <>
    <p className="pe-intro">{inbox ? "Check the inbox for messages that need your attention." : "Gather the day’s useful findings into a short briefing."}</p>
    <label className="pe-schedule-toggle"><span>Schedule enabled<small>{enabled ? "Future runs are scheduled" : "Future runs are paused"}</small></span><input type="checkbox" role="switch" checked={enabled} onChange={e => onChange({ ...draft, ...(inbox ? { inboxEnabled: e.target.checked } : { briefEnabled: e.target.checked }) })} /></label>
    <dl className="pe-facts"><div><dt>Agent</dt><dd>{draft.name}</dd></div><div><dt>Repeats</dt><dd>{inbox ? "Every 30 minutes" : "Every day"}</dd></div><div><dt>Timezone</dt><dd>Asia/Kolkata</dd></div></dl>
    {!inbox && <label className="pe-field" htmlFor="pe-brief-time">Run at · IST<input id="pe-brief-time" type="time" required value={draft.briefTime} onChange={e => onChange({ ...draft, briefTime: e.target.value })} /></label>}
    <p className="pe-note">Pausing future runs does not interrupt work already running.</p>
  </>;
}

function WorkDetail({ part, name }: { part: ProfilePart; name: string }) {
  if (part === "current") return <>
    <h3 className="pe-work-title">Preparing morning brief</h3>
    <p className="pe-current-stage">Reading sources</p>
    <dl className="pe-facts"><div><dt>Started</dt><dd>09:40 IST</dd></div><div><dt>Agent</dt><dd>{name}</dd></div></dl>
    <section className="pe-detail"><h3>Latest activity</h3><p>Reading the original sources for items selected for the brief.</p></section>
  </>;
  if (part === "recent") return <>
    <h3 className="pe-work-title">Brief delivered</h3>
    <dl className="pe-facts"><div><dt>Finished</dt><dd>Yesterday · 19:00 IST</dd></div><div><dt>Outcome</dt><dd>6 items summarized</dd></div></dl>
    <p className="pe-intro">Four research updates and two inbox items, with source links and items needing your attention.</p>
  </>;
  if (part === "runtime") return <>
    <dl className="pe-facts"><div><dt>Runtime</dt><dd>Renoa agent loop</dd></div><div><dt>Execution</dt><dd>Host-managed VPS</dd></div></dl>
    <p className="pe-note">Example runtime. Runtime selection is not available in this preview.</p>
  </>;
  return null;
}

export function ProfileEditor(props: EditorProps) {
  const { part, draft, onChange, onNavigate } = props;
  return <div className="pe-editor">
    {part === "setup" ? <SetupEditor draft={draft} onChange={onChange} onNavigate={onNavigate} />
      : part === "model" ? <ModelEditor draft={draft} onChange={onChange} />
        : part === "instructions" ? <InstructionsEditor draft={draft} onChange={onChange} />
          : part === "tools" ? <CapabilitiesEditor draft={draft} onChange={onChange} />
            : part === "inbox" || part === "brief" ? <Schedule {...props} />
              : <WorkDetail part={part} name={draft.name} />}
  </div>;
}
