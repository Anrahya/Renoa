import { useState, type FormEvent } from "react";
import { Fingerprint, ArrowRight } from "@phosphor-icons/react";
import { authenticatePasskey, registerPasskey } from "./passkeys";
import { pairBrowser } from "./browser-pairing";

export function HostLogin({ refresh, forbidden }: { refresh: () => void; forbidden: boolean }) {
  const [method, setMethod] = useState<"pair" | "passkey" | "register">("pair");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function signIn(event: FormEvent) {
    event.preventDefault();
    setBusy(true); setError(null);
    try {
      if (method === "pair") await pairBrowser(token.trim());
      else if (method === "register") await registerPasskey(token.trim());
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
  return <main id="host-main" className="host-login host-content">
    <h1>{method === "passkey" ? "Welcome back" : "Connect to your Host"}</h1>
    <p className="host-intro">Pair this browser to open your agents, shared connections, and work.</p>
    <form onSubmit={event => void signIn(event)}>
      {forbidden && <p className="host-notice">Your current login belongs to another owner. Pair or sign in as this Host’s owner.</p>}
      {method !== "passkey" && <label className="host-token">{method === "pair" ? "One-time pairing code" : "Passkey setup code"}<input type="password" autoComplete="off" required value={token}
        onChange={event => setToken(event.target.value)} placeholder="From your Host’s setup command" /></label>}
      {error && <p role="alert" className="host-error">{error}</p>}
      <button className="host-primary" type="submit" disabled={busy}><Fingerprint size={20} />
        {busy ? (method === "pair" ? "Pairing this browser…" : "Waiting for your passkey…")
          : method === "pair" ? "Pair this browser" : method === "register" ? "Create a passkey" : "Continue with passkey"}<ArrowRight size={18} /></button>
    </form>
    <p className="host-caption">This browser stays signed in across visits and connection changes.</p>
    <div className="host-login-methods">{(["pair", "passkey", "register"] as const).filter(value => value !== method).map(value =>
      <button key={value} className="host-link" disabled={busy} onClick={() => { setMethod(value); setError(null); setToken(""); }}>
        {value === "pair" ? "Use a Host pairing code" : value === "passkey" ? "Sign in with a passkey" : "Set up a passkey on this device"}
      </button>)}</div>
  </main>;
}
