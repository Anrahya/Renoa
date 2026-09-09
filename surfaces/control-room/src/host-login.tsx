import { useState, type FormEvent } from "react";
import { Fingerprint, ArrowRight } from "@phosphor-icons/react";
import { authenticatePasskey, registerPasskey } from "./passkeys";

export function HostLogin({ refresh, forbidden }: { refresh: () => void; forbidden: boolean }) {
  const [register, setRegister] = useState(false);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function signIn(event: FormEvent) {
    event.preventDefault();
    setBusy(true); setError(null);
    try {
      if (register) await registerPasskey(token.trim());
      else {
        const access = await fetch("/v1/host/access", { cache: "no-store", credentials: "same-origin" });
        if (!access.ok) throw new Error("The Host is unavailable. Try again when it returns.");
        const value: unknown = await access.json();
        if (typeof value !== "object" || value === null || !("owner_principal_id" in value) || typeof value.owner_principal_id !== "string")
          throw new Error("The Host has not provided its login configuration.");
        await authenticatePasskey(value.owner_principal_id);
      }
      setToken(""); refresh();
    } catch (failure) { setError(failure instanceof Error ? failure.message : "Sign-in failed."); }
    finally { setBusy(false); }
  }
  return <main className="host-login host-content">
    <p className="host-kicker">Your system, within reach</p>
    <h1>{register ? "Make yourself at home." : "Welcome back."}</h1>
    <p className="host-intro">One Host. Every agent. A place to see the work and what comes next.</p>
    <form onSubmit={event => void signIn(event)}>
      {forbidden && <p className="host-notice">Your current login belongs to another owner. Sign in with this Host’s passkey.</p>}
      {register && <label className="host-token">One-time setup token<input type="password" autoComplete="off" required value={token}
        onChange={event => setToken(event.target.value)} placeholder="From your Host’s setup command" /></label>}
      {error && <p role="alert" className="host-error">{error}</p>}
      <button className="host-primary" type="submit" disabled={busy}><Fingerprint size={20} />
        {busy ? "Waiting for your passkey…" : register ? "Create a passkey" : "Continue with passkey"}<ArrowRight size={18} /></button>
    </form>
    <p className="host-caption">This browser stays signed in across visits and connection changes.</p>
    <button className="host-link" disabled={busy} onClick={() => { setRegister(!register); setError(null); }}>
      {register ? "I already have a passkey" : "First time here? Set up your passkey"}</button>
  </main>;
}
