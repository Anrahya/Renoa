"use strict";

const statusNode = document.querySelector("#status");
const form = document.querySelector("#form");
const relayId = location.pathname.split("/").at(-2);
const secret = new URLSearchParams(location.hash.slice(1));
const version = secret.get("v");
const keyHex = secret.get("key");
const capability = secret.get("token");
const expectedIssuer = secret.get("issuer");
history.replaceState(null, "", location.pathname);

function bytes(hex, size) {
  if (!hex || hex.length !== size * 2 || !/^[0-9a-f]+$/.test(hex)) throw new Error("This setup link is invalid.");
  return Uint8Array.from(hex.match(/../g), part => Number.parseInt(part, 16));
}

function hex(value) {
  return Array.from(new Uint8Array(value), byte => byte.toString(16).padStart(2, "0")).join("");
}

function field(name, label, type = "text", required = true) {
  const wrapper = document.createElement("label");
  wrapper.textContent = label;
  const input = document.createElement("input");
  input.name = name;
  input.type = type;
  input.required = required;
  input.autocomplete = "off";
  wrapper.append(input);
  return wrapper;
}

async function start() {
  if (version !== "1") throw new Error("This setup link is invalid.");
  const keyBytes = bytes(keyHex, 32);
  bytes(capability, 32);
  const response = await fetch(`/v1/credential-relays/${relayId}/form`, { cache: "no-store" });
  if (!response.ok) throw new Error("This setup link is invalid or expired.");
  const metadata = await response.json();
  statusNode.textContent = `Store ${metadata.credentialId} on its requesting Renoa Host.`;
  if (metadata.kind === "api_token") {
    form.append(field("value", "API key or token", "password"));
  } else if (metadata.kind === "oauth_client") {
    if (!expectedIssuer) throw new Error("This OAuth setup link is invalid.");
    const provider = document.createElement("p");
    provider.textContent = `OAuth provider: ${expectedIssuer}`;
    form.append(provider);
    form.append(field("client_id", "Client ID"));
    form.append(field("client_secret", "Client secret (optional)", "password", false));
  } else {
    throw new Error("This credential type is unsupported.");
  }
  const note = document.createElement("small");
  note.textContent = "Encrypted in this browser. renoa.live cannot decrypt it.";
  const button = document.createElement("button");
  button.type = "submit";
  button.textContent = "Save to Renoa";
  form.append(note, button);
  form.hidden = false;
  form.addEventListener("submit", async event => {
    event.preventDefault();
    button.disabled = true;
    statusNode.textContent = "Encrypting…";
    try {
      const values = Object.fromEntries(new FormData(form));
      const payload = metadata.kind === "api_token"
        ? { schema_version: 1, value: values.value }
        : { schema_version: 1, issuer: expectedIssuer, client_id: values.client_id, ...(values.client_secret ? { client_secret: values.client_secret } : {}) };
      const nonce = crypto.getRandomValues(new Uint8Array(12));
      const key = await crypto.subtle.importKey("raw", keyBytes, "AES-GCM", false, ["encrypt"]);
      const aad = new TextEncoder().encode(`renoa credential relay v2\0${relayId}\0${metadata.credentialId}\0${metadata.kind}\0${expectedIssuer ?? ""}`);
      const ciphertext = await crypto.subtle.encrypt(
        { name: "AES-GCM", iv: nonce, additionalData: aad },
        key,
        new TextEncoder().encode(JSON.stringify(payload)),
      );
      const saved = await fetch(`/v1/credential-relays/${relayId}/submit`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ version: 1, capability, nonce: hex(nonce), ciphertext: hex(ciphertext) }),
      });
      if (!saved.ok) throw new Error("Renoa did not accept this credential. The link may have expired.");
      for (const input of form.querySelectorAll("input")) input.value = "";
      form.remove();
      statusNode.textContent = "Saved. You can close this tab.";
    } catch (error) {
      statusNode.textContent = error instanceof Error ? error.message : "Credential setup failed.";
      button.disabled = false;
    }
  });
}

start().catch(error => {
  statusNode.textContent = error instanceof Error ? error.message : "Credential setup failed.";
});
