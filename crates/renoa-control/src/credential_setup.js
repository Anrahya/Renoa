"use strict";

// The decryption key and upload token travel in the link's fragment, which browsers never send to a
// server. Read them, then take them out of the address bar before anything else happens.
const relayId = location.pathname.split("/").at(-2);
const link = new URLSearchParams(location.hash.slice(1));
history.replaceState(null, "", location.pathname);

const page = document.querySelector("#page");
const form = document.querySelector("#form");
const send = document.querySelector("#send");
const retry = document.querySelector("#retry");
const subject = document.querySelector("#subject");
const expiry = document.querySelector("#expiry");
const views = [...document.querySelectorAll("[data-view]")];
const errors = [...document.querySelectorAll("[data-error]")];
const inputs = {
  value: document.querySelector("#value"),
  clientId: document.querySelector("#client-id"),
  clientSecret: document.querySelector("#client-secret"),
};
// The design system marks a field that failed with data-invalid on its wrapper.
const fields = {
  value: document.querySelector("#field-value"),
  clientId: document.querySelector("#field-client-id"),
};

// The parsed link, or null when it is incomplete. start() adds the kind and id the coordinator reports.
const setup = readLink();
// The last value sealed, so a retry after a lost answer sends the same bytes (see seal).
let sealed = null;
let sending = false;

function hexBytes(hex, size) {
  if (!hex || hex.length !== size * 2 || !/^[0-9a-f]+$/.test(hex)) return null;
  return Uint8Array.from(hex.match(/../g), part => Number.parseInt(part, 16));
}

function hex(value) {
  return Array.from(new Uint8Array(value), byte => byte.toString(16).padStart(2, "0")).join("");
}

function readLink() {
  const key = hexBytes(link.get("key"), 32);
  const capability = link.get("token");
  if (link.get("v") !== "1" || !key || !hexBytes(capability, 32)) return null;
  return { key, capability, issuer: link.get("issuer") };
}

function issuerHost(issuer) {
  if (!issuer) return null;
  try {
    return new URL(issuer).host || null;
  } catch {
    return null;
  }
}

function clock(milliseconds) {
  const date = new Date(milliseconds);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

function show(view) {
  page.dataset.view = view;
  for (const section of views) section.hidden = section.dataset.view !== view;
  if (view === "ready") {
    (setup.kind === "api_token" ? inputs.value : inputs.clientId).focus();
  } else if (view !== "loading") {
    views.find(section => section.dataset.view === view).focus();
  }
}

function showError(name) {
  for (const error of errors) error.hidden = error.dataset.error !== name;
}

function clearErrors() {
  showError(null);
  for (const input of Object.values(inputs)) input.removeAttribute("aria-invalid");
  for (const field of Object.values(fields)) field.removeAttribute("data-invalid");
}

function invalid(name, key) {
  showError(name);
  fields[key].setAttribute("data-invalid", "");
  inputs[key].setAttribute("aria-invalid", "true");
  inputs[key].focus();
  return null;
}

// Nothing typed stays in the page once the link has been used up or has stopped working.
function finish(view) {
  for (const input of Object.values(inputs)) input.value = "";
  sealed = null;
  show(view);
}

async function start() {
  if (!setup) return show("incomplete");
  if (!globalThis.crypto?.subtle) return show("browser");
  show("loading");
  let response;
  try {
    response = await fetch(`/v1/credential-relays/${relayId}/form`, { cache: "no-store" });
  } catch {
    return show("unavailable");
  }
  if (response.status === 404) return show("expired");
  if (!response.ok) return show("unavailable");
  const metadata = await response.json().catch(() => null);
  if (!metadata) return show("unavailable");
  if (metadata.kind !== "api_token" && metadata.kind !== "oauth_client") return show("kind");
  const host = metadata.kind === "oauth_client" ? issuerHost(setup.issuer) : null;
  if (metadata.kind === "oauth_client" && !host) return show("incomplete");
  setup.kind = metadata.kind;
  setup.credentialId = metadata.credentialId;
  page.dataset.kind = metadata.kind;
  subject.textContent = metadata.kind === "api_token" ? metadata.credentialId : host;
  expiry.textContent = clock(metadata.expiresAtMs);
  inputs.value.disabled = metadata.kind !== "api_token";
  inputs.clientId.disabled = metadata.kind !== "oauth_client";
  inputs.clientSecret.disabled = metadata.kind !== "oauth_client";
  show("ready");
}

// The plaintext the Host expects for this kind, or null (with the reason shown) when a field is blank.
function collect() {
  const read = input => input.value.trim();
  if (setup.kind === "api_token") {
    const value = read(inputs.value);
    if (!value) return invalid("empty-value", "value");
    return JSON.stringify({ schema_version: 1, value });
  }
  const clientId = read(inputs.clientId);
  if (!clientId) return invalid("empty-client-id", "clientId");
  const clientSecret = read(inputs.clientSecret);
  return JSON.stringify({
    schema_version: 1,
    issuer: setup.issuer,
    client_id: clientId,
    ...(clientSecret ? { client_secret: clientSecret } : {}),
  });
}

// AES-GCM must never see one nonce with two plaintexts. A retry of the same value reuses the nonce and
// so produces the same ciphertext, which the coordinator accepts as the submission it already holds;
// any other value gets a fresh nonce.
async function seal(plaintext) {
  if (sealed?.plaintext === plaintext) return sealed;
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const key = await crypto.subtle.importKey("raw", setup.key, "AES-GCM", false, ["encrypt"]);
  const aad = new TextEncoder().encode(
    `renoa credential relay v2\0${relayId}\0${setup.credentialId}\0${setup.kind}\0${setup.issuer ?? ""}`,
  );
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv: nonce, additionalData: aad },
    key,
    new TextEncoder().encode(plaintext),
  );
  sealed = { plaintext, nonce: hex(nonce), ciphertext: hex(ciphertext) };
  return sealed;
}

async function submit(plaintext) {
  let body;
  try {
    const { nonce, ciphertext } = await seal(plaintext);
    body = JSON.stringify({ version: 1, capability: setup.capability, nonce, ciphertext });
  } catch {
    return showError("encrypt");
  }
  let response;
  try {
    response = await fetch(`/v1/credential-relays/${relayId}/submit`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body,
    });
  } catch {
    return showError("unreachable");
  }
  if (response.ok) return finish("sent");
  if (response.status === 409) return finish("used");
  if ([400, 401, 404].includes(response.status)) return finish("expired");
  showError("unreachable");
}

form.addEventListener("submit", async event => {
  event.preventDefault();
  if (sending) return;
  clearErrors();
  const plaintext = collect();
  if (!plaintext) return;
  sending = true;
  send.disabled = true;
  form.setAttribute("aria-busy", "true");
  try {
    await submit(plaintext);
  } finally {
    sending = false;
    send.disabled = false;
    form.removeAttribute("aria-busy");
  }
});

const begin = () => start().catch(() => show("unavailable"));
retry.addEventListener("click", begin);
begin();
