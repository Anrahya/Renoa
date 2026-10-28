// Runs the credential setup page exactly as a browser would receive it: the markup and the script are
// fetched from a live coordinator, the script runs against a minimal DOM built from that markup, and its
// requests go to the same coordinator. Prints one JSON report on stdout for the Rust test to judge.
//
// The DOM is deliberately small. It knows elements by id and by data-view / data-error, and it throws on
// any selector the page does not already use, so the markup and the script cannot drift apart unnoticed.
import vm from "node:vm";

const scenario = JSON.parse(process.argv[2]);
const { origin, relayId, fragment, faults = [], steps = [] } = scenario;
const pagePath = `/v1/credential-relays/${relayId}/setup`;

const events = [];
const requests = [];
const consoleCalls = [];
const unhandled = [];
const focusLog = [];
const viewLog = [];
let pending = 0;
let activity = 0;

const realFetch = globalThis.fetch;
const realCrypto = globalThis.crypto;
const html = await (await realFetch(origin + pagePath)).text();
const script = await (await realFetch(origin + "/v1/credential-setup.js")).text();

function camel(name) {
  return name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function element(attributes) {
  const listeners = {};
  const dataset = {};
  for (const [name, value] of Object.entries(attributes)) {
    if (name.startsWith("data-")) dataset[camel(name.slice(5))] = value;
  }
  const node = {
    id: attributes.id ?? null,
    hidden: "hidden" in attributes,
    disabled: false,
    value: "",
    textContent: "",
    attributes: { ...attributes },
    listeners,
    addEventListener(type, handler) {
      (listeners[type] ??= []).push(handler);
    },
    setAttribute(name, value) {
      node.attributes[name] = String(value);
    },
    removeAttribute(name) {
      delete node.attributes[name];
    },
    focus() {
      focusLog.push(node.id ?? `view:${node.dataset.view}`);
    },
  };
  node.dataset = new Proxy(dataset, {
    set(target, property, value) {
      target[property] = value;
      if (node.id === "page" && property === "view") viewLog.push(value);
      return true;
    },
  });
  return node;
}

const nodes = [];
for (const match of html.matchAll(/<([a-z][a-z0-9]*)\b([^>]*)>/gi)) {
  const attributes = {};
  for (const attribute of match[2].matchAll(/([a-zA-Z_:][-a-zA-Z0-9_:.]*)(?:="([^"]*)")?/g)) {
    attributes[attribute[1]] = attribute[2] ?? "";
  }
  if ("id" in attributes || "data-view" in attributes || "data-error" in attributes) {
    nodes.push(element(attributes));
  }
}
const byId = id => nodes.find(node => node.id === id) ?? null;

globalThis.document = {
  querySelector(selector) {
    if (!selector.startsWith("#")) throw new Error(`unsupported selector ${selector}`);
    return byId(selector.slice(1));
  },
  querySelectorAll(selector) {
    const named = /^\[data-([a-z]+)\]$/.exec(selector);
    if (!named) throw new Error(`unsupported selector ${selector}`);
    return nodes.filter(node => `data-${named[1]}` in node.attributes);
  },
};
globalThis.location = { pathname: pagePath, hash: fragment ? `#${fragment}` : "" };
globalThis.history = {
  replaceState(_state, _title, url) {
    events.push({ type: "replaceState", url });
  },
};
globalThis.console = new Proxy(
  {},
  {
    get: (_target, method) => (...args) => consoleCalls.push({ method, args: args.map(String) }),
  },
);
process.on("unhandledRejection", reason => unhandled.push(String(reason)));

function tracked(promise) {
  pending += 1;
  activity += 1;
  return promise.finally(() => {
    pending -= 1;
    activity += 1;
  });
}

const subtle = scenario.webCrypto === false ? undefined : {
  importKey: (...args) => tracked(realCrypto.subtle.importKey(...args)),
  encrypt: (...args) =>
    tracked(scenario.failEncrypt ? Promise.reject(new Error("blocked")) : realCrypto.subtle.encrypt(...args)),
};
Object.defineProperty(globalThis, "crypto", {
  configurable: true,
  writable: true,
  value: { getRandomValues: array => realCrypto.getRandomValues(array), subtle },
});

const attempts = {};
globalThis.fetch = (input, init = {}) =>
  tracked(
    (async () => {
      const url = String(input);
      const kind = url.endsWith("/form") ? "form" : url.endsWith("/submit") ? "submit" : "other";
      attempts[kind] = (attempts[kind] ?? 0) + 1;
      requests.push({ kind, method: init.method ?? "GET", url, body: init.body ?? null });
      events.push({ type: "fetch", kind });
      const fault = faults.find(entry => entry.path === kind && entry.attempt === attempts[kind]);
      if (fault?.action === "network") throw new TypeError("fetch failed");
      if (fault?.action === "status") return new Response("{}", { status: fault.status });
      if (fault?.action === "json") {
        return new Response(JSON.stringify(fault.body), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }
      const answer = await realFetch(new URL(url, origin), init);
      const body = await answer.arrayBuffer();
      if (fault?.action === "lost_answer") throw new TypeError("fetch failed");
      return new Response(body, { status: answer.status, headers: answer.headers });
    })(),
  );

async function settle() {
  let quiet = 0;
  while (quiet < 3) {
    const before = activity;
    await new Promise(resolve => setImmediate(resolve));
    quiet = pending === 0 && activity === before ? quiet + 1 : 0;
  }
}

function snapshot() {
  const page = byId("page");
  const named = key => nodes.filter(node => `data-${key}` in node.attributes);
  return {
    view: page?.dataset.view ?? null,
    kind: page?.dataset.kind ?? null,
    visibleViews: named("view").filter(node => !node.hidden).map(node => node.dataset.view),
    visibleErrors: named("error").filter(node => !node.hidden).map(node => node.dataset.error),
    inputs: Object.fromEntries(["value", "client-id", "client-secret"].map(id => [id, byId(id).value])),
    ariaInvalid: nodes.filter(node => node.attributes["aria-invalid"] === "true").map(node => node.id),
    fieldInvalid: nodes.filter(node => "data-invalid" in node.attributes).map(node => node.id),
    sendDisabled: byId("send").disabled,
    busy: byId("form").attributes["aria-busy"] ?? null,
    subject: byId("subject").textContent,
    expiry: byId("expiry").textContent,
    requestCount: requests.length,
  };
}

let scriptError = null;
const snapshots = [];
try {
  vm.runInThisContext(script, { filename: "credential-setup.js" });
  await settle();
  snapshots.push(snapshot());
  for (const step of steps) {
    if (step.fill) {
      for (const [id, value] of Object.entries(step.fill)) byId(id).value = value;
    } else if (step.submit) {
      const event = { preventDefault() {} };
      await Promise.all(byId("form").listeners.submit.map(handler => handler(event)));
    } else if (step.click) {
      await Promise.all(byId(step.click).listeners.click.map(handler => handler({})));
    }
    await settle();
    snapshots.push(snapshot());
  }
} catch (error) {
  scriptError = String(error?.stack ?? error);
}

process.stdout.write(
  JSON.stringify({ scriptError, unhandled, events, views: viewLog, requests, consoleCalls, focusLog, snapshots }),
);
process.exit(0);
