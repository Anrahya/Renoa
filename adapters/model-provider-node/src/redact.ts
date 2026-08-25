const PROVIDER_MESSAGE_LIMIT = 512;

const SENSITIVE_FIELDS = new Set([
  "authorization",
  "proxyauthorization",
  "cookie",
  "setcookie",
  "credential",
  "credentials",
  "secret",
  "clientsecret",
  "password",
  "passwd",
  "apikey",
  "xapikey",
  "accesskey",
  "accesskeyid",
  "secretkey",
  "secretaccesskey",
  "privatekey",
  "token",
  "accesstoken",
  "refreshtoken",
  "idtoken",
  "bearertoken",
  "sessiontoken",
  "authtoken",
  "xauthtoken",
  "xaccesstoken",
  "csrftoken",
  "xcsrftoken",
  "xxsrftoken",
]);

const SENSITIVE_QUERY = new Set([
  ...SENSITIVE_FIELDS,
  "signature",
  "sig",
  "xamzsignature",
  "xamzcredential",
  "xamzsecuritytoken",
]);

const SENSITIVE_ASSIGNMENT =
  /\b(access_token|refresh_token|id_token|client_secret|api_key|private_key|password|passwd)\s*=\s*(?:\\*"(?:\\.|[^"\\])*\\*"|\\*'(?:\\.|[^'\\])*\\*'|[^\s&]+)/gi;

const PRIVATE_KEY_PEM =
  /-----BEGIN [A-Z0-9 ]{0,80}PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z0-9 ]{0,80}PRIVATE KEY-----|$)/g;

const MALFORMED_SENSITIVE_FIELD =
  /\b(access[_-]?token|refresh[_-]?token|id[_-]?token|client[_-]?secret|api[_-]?key|x-api-key|private[_-]?key|password|passwd)\b(?:\\?["'])?\s*[:=]\s*[^\r\n]*/gi;

export function redactSecrets(value: unknown): unknown {
  return redactValue(value, undefined);
}

export function redactHeaders(headers: Readonly<Record<string, string>>): Record<string, string> {
  return Object.fromEntries(
    Object.entries(headers).map(([name, value]) => [
      name,
      isSensitiveName(name) ? "<redacted>" : redactText(value),
    ]),
  );
}

export function redactText(value: string): string {
  return redactAuthTokens(
    redactAssignments(
      redactMalformedSensitiveFields(
        redactPrivateKeys(redactEmbeddedJson(redactSignedUrls(value))),
      ),
    ),
  );
}

export function boundProviderMessage(value: string): string {
  const redacted = redactText(value).trim();
  if (redacted.length <= PROVIDER_MESSAGE_LIMIT) {
    return redacted;
  }
  return redacted.slice(0, PROVIDER_MESSAGE_LIMIT);
}

function redactValue(value: unknown, key: string | undefined): unknown {
  if (key !== undefined && isSensitiveName(key)) {
    return "<redacted>";
  }
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
      try {
        return JSON.stringify(redactSecrets(JSON.parse(trimmed)));
      } catch {
        // The string is not JSON; apply the same text redaction as logs.
      }
    }
    return redactText(value);
  }
  if (Array.isArray(value)) {
    return value.map((entry) => redactValue(entry, undefined));
  }
  if (typeof value === "object" && value !== null) {
    return Object.fromEntries(
      Object.entries(value).map(([nestedKey, nested]) => [nestedKey, redactValue(nested, nestedKey)]),
    );
  }
  return value;
}

function redactAuthTokens(value: string): string {
  return value
    .replace(/\bBearer\s+[^\s"']+/gi, "Bearer <redacted>")
    .replace(/\bBasic\s+[^\s"']+/gi, "Basic <redacted>")
    .replace(/\bApiKey\s+[^\s"']+/gi, "ApiKey <redacted>")
    .replace(/\bAuthorization:\s*ApiKey\s+[^\s"']+/gi, "Authorization: ApiKey <redacted>")
    .replace(/\bCookie:\s*[^\r\n]*/gi, "Cookie: <redacted>");
}

function redactAssignments(value: string): string {
  return value.replace(SENSITIVE_ASSIGNMENT, (match) => {
    const separator = match.search(/\s*=/);
    return `${match.slice(0, separator)}=<redacted>`;
  });
}

function redactPrivateKeys(value: string): string {
  return value.replace(PRIVATE_KEY_PEM, "<redacted-private-key>");
}

function redactMalformedSensitiveFields(value: string): string {
  return value
    .split(/(\r\n|\r|\n)/)
    .map((line, index) => (index % 2 === 1 ? line : redactMalformedSensitiveLine(line)))
    .join("");
}

function redactMalformedSensitiveLine(line: string): string {
  const trimmed = line.trim();
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    try {
      JSON.parse(trimmed);
      return line;
    } catch {
      // A malformed JSON-looking diagnostic still needs conservative redaction.
    }
  }
  return line.replace(MALFORMED_SENSITIVE_FIELD, (match) => {
    const separator = match.search(/[:=]/);
    return `${match.slice(0, separator)}=<redacted>`;
  });
}

function redactEmbeddedJson(text: string): string {
  const trimmed = text.trim();
  try {
    return JSON.stringify(redactSecrets(JSON.parse(trimmed)));
  } catch {
    // The message is not itself JSON; redact JSON objects embedded in it.
  }
  let output = "";
  let index = 0;
  while (index < text.length) {
    const character = text[index];
    if (character === "{" || character === "[") {
      const parsed = parseJsonAt(text, index);
      if (parsed !== undefined) {
        output += JSON.stringify(redactSecrets(parsed.value));
        index = parsed.end;
        continue;
      }
    }
    output += character;
    index += 1;
  }
  return output;
}

function parseJsonAt(text: string, start: number): { value: unknown; end: number } | undefined {
  const opening = text[start];
  if (opening !== "{" && opening !== "[") {
    return undefined;
  }
  let depth = 0;
  let inString = false;
  let escape = false;
  for (let index = start; index < text.length; index += 1) {
    const character = text[index];
    if (inString) {
      if (escape) {
        escape = false;
        continue;
      }
      if (character === "\\") {
        escape = true;
        continue;
      }
      if (character === '"') {
        inString = false;
      }
      continue;
    }
    if (character === '"') {
      inString = true;
      continue;
    }
    if (character === "{" || character === "[") {
      depth += 1;
    } else if (character === "}" || character === "]") {
      depth -= 1;
      if (depth === 0) {
        try {
          return { value: JSON.parse(text.slice(start, index + 1)), end: index + 1 };
        } catch {
          return undefined;
        }
      }
    }
  }
  return undefined;
}

function isSensitiveName(name: string): boolean {
  return SENSITIVE_FIELDS.has(normalizeName(name));
}

function normalizeName(name: string): string {
  return name.toLowerCase().replace(/[_-]/g, "");
}

function redactSignedUrls(value: string): string {
  return value.replace(/\bhttps?:\/\/[^\s"'<>]+/gi, (url) => redactUrl(url));
}

function redactUrl(url: string): string {
  const queryIndex = url.indexOf("?");
  if (queryIndex === -1) {
    return url;
  }
  const originAndPath = url.slice(0, queryIndex);
  const rest = url.slice(queryIndex + 1);
  const hashIndex = rest.indexOf("#");
  const query = hashIndex === -1 ? rest : rest.slice(0, hashIndex);
  const hash = hashIndex === -1 ? "" : rest.slice(hashIndex);
  const params = query.split("&").map((pair) => {
    const separator = pair.indexOf("=");
    const rawName = separator === -1 ? pair : pair.slice(0, separator);
    if (SENSITIVE_QUERY.has(normalizeName(decodeQueryComponent(rawName)))) {
      return `${rawName}=<redacted>`;
    }
    return pair;
  });
  return `${originAndPath}?${params.join("&")}${hash}`;
}

function decodeQueryComponent(value: string): string {
  try {
    return decodeURIComponent(value.replace(/\+/g, " "));
  } catch {
    return value;
  }
}
