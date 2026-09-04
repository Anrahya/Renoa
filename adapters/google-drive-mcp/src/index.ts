import { createMcpHandler } from "agents/mcp/server";

import {
  GOOGLE_DRIVE_FILE_SCOPE,
  GOOGLE_DRIVE_READONLY_SCOPE,
  GOOGLE_DRIVE_SCOPE,
  GOOGLE_ISSUER,
  MCP_ENDPOINT,
  MCP_ORIGIN,
  RESOURCE_METADATA_PATH,
} from "./constants.js";
import type { DriveFetch } from "./drive-client.js";
import { bearerToken, createDriveServer } from "./server.js";

const ROOT_RESOURCE_METADATA_PATH = "/.well-known/oauth-protected-resource";

export interface DriveWorker {
  fetch(request: Request, env: Env, context: ExecutionContext): Promise<Response>;
}

export function createWorker(fetchFn: DriveFetch = fetch): DriveWorker {
  const mcp = createMcpHandler(
    (context) => createDriveServer(context, fetchFn),
    {
      route: "/mcp",
      corsOptions: false,
      allowedHostnames: ["drive.renoa.live"],
      allowedOriginHostnames: ["drive.renoa.live"],
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
        return Response.json({ status: "ok", service: "renoa-google-drive-mcp" });
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
        GOOGLE_DRIVE_SCOPE,
        GOOGLE_DRIVE_READONLY_SCOPE,
        GOOGLE_DRIVE_FILE_SCOPE,
      ],
      bearer_methods_supported: ["header"],
      resource_name: "Renoa Google Drive",
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
        "WWW-Authenticate": `Bearer resource_metadata="${metadata}", scope="${GOOGLE_DRIVE_SCOPE}"`,
        "X-Content-Type-Options": "nosniff",
      },
    },
  );
}

export default createWorker() satisfies ExportedHandler<Env>;
