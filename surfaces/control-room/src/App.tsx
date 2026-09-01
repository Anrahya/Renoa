import { lazy, Suspense, useState, type FormEvent } from "react";
import {
  Fingerprint,
  Key,
  LockKey,
  ShieldCheck,
  WarningCircle,
} from "@phosphor-icons/react";

import { useControlRoom } from "./use-control-room";
import { Workspace } from "./workspace";

const PreviewApp = import.meta.env.DEV
  ? lazy(async () => import("./preview-app"))
  : null;

export function App() {
  const previewEnabled =
    import.meta.env.DEV && new URLSearchParams(window.location.search).has("preview");
  if (previewEnabled && PreviewApp !== null) {
    return (
      <Suspense fallback={null}>
        <PreviewApp />
      </Suspense>
    );
  }
  return <LiveApp />;
}

function LiveApp() {
  const control = useControlRoom();
  if (control.connection === "connected" || control.connection === "disconnected") {
    return <Workspace control={control} />;
  }
  return <LockScreen control={control} />;
}

function LockScreen({ control }: { readonly control: ReturnType<typeof useControlRoom> }) {
  const [setup, setSetup] = useState(control.savedPrincipalId === "");
  const [principalId, setPrincipalId] = useState(control.savedPrincipalId);
  const [bootstrapToken, setBootstrapToken] = useState("");

  async function submit(event: FormEvent): Promise<void> {
    event.preventDefault();
    if (principalId.trim() === "") {
      return;
    }
    if (setup) {
      if (bootstrapToken.trim() === "") {
        return;
      }
      await control.register(principalId.trim(), bootstrapToken.trim());
    } else {
      await control.unlock(principalId.trim());
    }
  }

  return (
    <main className="lock-screen">
      <section className="lock-visual" aria-label="Renoa Control Room">
        <div className="lock-brand">renoa<span /></div>
        <img src="/assets/task-console.png" alt="Renoa task console" />
        <div className="visual-caption">
          <span className="live-dot" />
          <div><strong>Control Room</strong><small>Durable tasks, one clear view</small></div>
        </div>
      </section>

      <section className="lock-panel">
        <div className="lock-card">
          <div className="lock-icon"><LockKey size={26} weight="duotone" aria-hidden="true" /></div>
          <span className="eyebrow">Private control surface</span>
          <h1>{setup ? "Create your passkey" : "Unlock Renoa"}</h1>
          <p className="lock-intro">
            {setup
              ? "Use the one-use bootstrap from your Host. Renoa keeps no browser password."
              : "Your passkey proves who you are. The connection ticket stays in memory only."}
          </p>

          {control.error !== null && (
            <div className="form-error" role="alert">
              <WarningCircle size={18} weight="fill" aria-hidden="true" />
              <span>{control.error}</span>
            </div>
          )}

          <form onSubmit={(event) => void submit(event)}>
            <label htmlFor="principal-id">Principal ID</label>
            <div className="field-wrap">
              <Fingerprint size={19} aria-hidden="true" />
              <input
                id="principal-id"
                value={principalId}
                onChange={(event) => setPrincipalId(event.target.value)}
                placeholder="00000000-0000-0000-0000-000000000000"
                autoComplete="username webauthn"
                spellCheck={false}
                required
              />
            </div>
            {setup && (
              <>
                <label htmlFor="bootstrap-token">One-use bootstrap</label>
                <div className="field-wrap">
                  <Key size={19} aria-hidden="true" />
                  <input
                    id="bootstrap-token"
                    type="password"
                    value={bootstrapToken}
                    onChange={(event) => setBootstrapToken(event.target.value)}
                    placeholder="Paste bootstrap token"
                    autoComplete="off"
                    required
                  />
                </div>
              </>
            )}
            <button className="unlock-button" type="submit" disabled={control.busy}>
              <Fingerprint size={21} weight="bold" aria-hidden="true" />
              {control.busy ? "Waiting for passkey…" : setup ? "Create passkey" : "Continue with passkey"}
            </button>
          </form>

          <button className="mode-switch" onClick={() => setSetup((value) => !value)}>
            {setup ? "I already have a Renoa passkey" : "Set up this browser"}
          </button>
          <div className="security-note">
            <ShieldCheck size={18} weight="fill" aria-hidden="true" />
            <span>Same-origin WebAuthn · one-use RCP ticket · no session cookie</span>
          </div>
        </div>
      </section>
    </main>
  );
}
