import { useEffect, useRef } from "react";
import { X } from "@phosphor-icons/react";
import { ProfileEditor } from "./editor";
import { configurationParts, partTitles, profileScope, type ProfileDraft, type ProfilePart } from "./model";

interface Props {
  part: ProfilePart;
  profile: ProfileDraft;
  draft: ProfileDraft;
  dirty: boolean;
  onChange: (draft: ProfileDraft) => void;
  onSave: () => void;
  onDiscard: () => void;
  onNavigate: (part: ProfilePart) => void;
  onClose: () => void;
}

export function ProfileDrawer({ part, profile, draft, dirty, onChange, onSave, onDiscard, onNavigate, onClose }: Props) {
  const dialog = useRef<HTMLDialogElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const pointerStartedOutside = useRef(false);
  const scope = profileScope(part);
  const isConfiguration = scope === "agent";
  const invalid = isConfiguration
    ? !draft.name.trim() ? "Give the agent a name." : !draft.instructions.trim() ? "Add a purpose for this agent." : !Number.isInteger(draft.maxTokens) || draft.maxTokens < 1 || draft.maxTokens > 131072 ? "Output limit must be between 1 and 131,072 tokens." : ""
    : scope === "brief" && !draft.briefTime ? "Choose a run time." : "";
  useEffect(() => {
    const element = dialog.current;
    const opener = document.activeElement;
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    element?.showModal();
    return () => {
      element?.close();
      document.body.style.overflow = overflow;
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, []);
  useEffect(() => { body.current?.scrollTo(0, 0); }, [part]);
  function outside(x: number, y: number) {
    const rect = dialog.current?.getBoundingClientRect();
    return rect !== undefined && (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom);
  }
  return <dialog ref={dialog} className={`ap-drawer${isConfiguration ? " ap-configuration" : ""}`} aria-labelledby="ap-drawer-title"
    onCancel={event => { event.preventDefault(); onClose(); }}
    onPointerDown={event => { pointerStartedOutside.current = outside(event.clientX, event.clientY); }}
    onClick={event => { if (pointerStartedOutside.current && outside(event.clientX, event.clientY)) onClose(); pointerStartedOutside.current = false; }}>
    <form className="ap-drawer-form" onSubmit={event => { event.preventDefault(); if (dirty && !invalid) onSave(); }}>
      <header className="ap-drawer-header">
        <div><h2 id="ap-drawer-title">{isConfiguration ? `Customize ${profile.name}` : partTitles[part]}</h2><p className="ap-panel-scope">{scope ? "Preview · Changes stay in this tab" : "Preview · Example data"}</p></div>
        <button type="button" className="ap-icon-button" aria-label="Close panel" onClick={onClose} autoFocus><X size={22} /></button>
      </header>
      {isConfiguration && <nav className="ap-section-nav" aria-label="Agent configuration">
        {configurationParts.map(section => <button key={section} type="button" aria-current={part === section ? "page" : undefined} onClick={() => onNavigate(section)}>{partTitles[section]}</button>)}
      </nav>}
      <div className="ap-drawer-body" ref={body}><ProfileEditor part={part} draft={draft} onChange={onChange} onNavigate={onNavigate} /></div>
      {scope && <footer className="ap-drawer-footer">
        {invalid && <p className="ap-validation" role="alert">{invalid}</p>}
        <div className="ap-footer-actions"><span className="ap-draft-state">{dirty ? "Unsaved changes" : "No changes"}</span>
          {dirty && <button type="button" className="ap-text-button" onClick={onDiscard}>Discard</button>}
          <button className="ap-primary" disabled={!dirty || Boolean(invalid)}>Save changes</button>
        </div>
      </footer>}
    </form>
  </dialog>;
}
