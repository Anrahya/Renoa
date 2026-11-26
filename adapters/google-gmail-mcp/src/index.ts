import { createMcpHandler } from "agents/mcp/server";

import {
  GOOGLE_GMAIL_COMPOSE_SCOPE,
  GOOGLE_GMAIL_MODIFY_SCOPE,
  GOOGLE_GMAIL_READONLY_SCOPE,
  GOOGLE_GMAIL_SEND_SCOPE,
  GOOGLE_ISSUER,
  MCP_ENDPOINT,
  MCP_ORIGIN,
  RESOURCE_METADATA_PATH,
} from "./constants.js";
import type { GmailFetch } from "./gmail-client.js";
import { bearerToken, createGmailServer } from "./server.js";

const ROOT_RESOURCE_METADATA_PATH = "/.well-known/oauth-protected-resource";

export interface GmailWorker {
  fetch(request: Request, env: Env, context: ExecutionContext): Promise<Response>;
}

export function createWorker(fetchFn: GmailFetch = fetch): GmailWorker {
  const mcp = createMcpHandler(
    (context) => createGmailServer(context, fetchFn),
    {
      route: "/mcp",
      corsOptions: false,
      allowedHostnames: ["gmail.renoa.live"],
      allowedOriginHostnames: ["gmail.renoa.live"],
    },
  );

  return {
    async fetch(request, env, context): Promise<Response> {
      const url = new URL(request.url);
      if (
        url.pathname === RESOURCE_METADATA_PATH ||
        url.pathname === ROOT_RESOURCE_METADATA_PATH
      ) {
        return resourceMetadata();
      }
      if (url.pathname === "/healthz") {
        return Response.json({ status: "ok", service: "renoa-google-gmail-mcp" });
      }
      if (url.pathname !== "/mcp") {
        return Response.json({ error: "not_found" }, { status: 404 });
      }
      if (bearerToken(request.headers) === undefined) {
        return unauthorized();
      }
      return mcp(request, env, context);
    },
  };
}

function resourceMetadata(): Response {
  return Response.json(
    {
      resource: MCP_ENDPOINT,
      authorization_servers: [GOOGLE_ISSUER],
      scopes_supported: [
        GOOGLE_GMAIL_MODIFY_SCOPE,
        GOOGLE_GMAIL_READONLY_SCOPE,
        GOOGLE_GMAIL_COMPOSE_SCOPE,
        GOOGLE_GMAIL_SEND_SCOPE,
      ],
      bearer_methods_supported: ["header"],
      resource_name: "Renoa Google Gmail",
    },
    {
      headers: {
        "Cache-Control": "public, max-age=3600",
        "X-Content-Type-Options": "nosniff",
      },
    },
  );
}

function unauthorized(): Response {
  const metadata = `${MCP_ORIGIN}${RESOURCE_METADATA_PATH}`;
  return Response.json(
    { error: "authorization_required" },
    {
      status: 401,
      headers: {
        "Cache-Control": "no-store",
        "WWW-Authenticate": `Bearer resource_metadata="${metadata}", scope="${GOOGLE_GMAIL_MODIFY_SCOPE}"`,
        "X-Content-Type-Options": "nosniff",
      },
    },
  );
}

export default createWorker() satisfies ExportedHandler<Env>;
