import { createMcpHandler } from "agents/mcp/server";

import type { CalendarFetch } from "./calendar-client.js";
import {
  GOOGLE_CALENDAR_EVENTS_SCOPE,
  GOOGLE_CALENDAR_READONLY_SCOPE,
  GOOGLE_CALENDAR_SCOPE,
  GOOGLE_ISSUER,
  MCP_ENDPOINT,
  MCP_ORIGIN,
  RESOURCE_METADATA_PATH,
} from "./constants.js";
import { bearerToken, createCalendarServer } from "./server.js";

const ROOT_RESOURCE_METADATA_PATH = "/.well-known/oauth-protected-resource";

export interface CalendarWorker {
  fetch(request: Request, env: Env, context: ExecutionContext): Promise<Response>;
}

const defaultFetch: CalendarFetch = (input, init) => fetch(input, init);

export function createWorker(fetchFn: CalendarFetch = defaultFetch): CalendarWorker {
  const mcp = createMcpHandler(
    (context) => createCalendarServer(context, fetchFn),
    {
      route: "/mcp",
      corsOptions: false,
      allowedHostnames: ["calendar.renoa.live"],
      allowedOriginHostnames: ["calendar.renoa.live"],
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
        return Response.json({ status: "ok", service: "renoa-google-calendar-mcp" });
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
        GOOGLE_CALENDAR_SCOPE,
        GOOGLE_CALENDAR_READONLY_SCOPE,
        GOOGLE_CALENDAR_EVENTS_SCOPE,
      ],
      bearer_methods_supported: ["header"],
      resource_name: "Renoa Google Calendar",
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
        "WWW-Authenticate": `Bearer resource_metadata="${metadata}", scope="${GOOGLE_CALENDAR_SCOPE}"`,
        "X-Content-Type-Options": "nosniff",
      },
    },
  );
}

export default createWorker() satisfies ExportedHandler<Env>;
