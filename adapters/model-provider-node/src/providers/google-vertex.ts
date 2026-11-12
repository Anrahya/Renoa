import type { Api, Model } from "@earendil-works/pi-ai";

import type { Credential } from "../credentials.js";

/**
 * The endpoint template of every Vertex entry in Pi's catalog
 * (`packages/ai/src/providers/google-vertex.ts`, v1.5.0). It is the binding's
 * base URL; the route a request takes comes from the stored credential.
 */
export const VERTEX_TEMPLATE_BASE_URL = "https://{location}-aiplatform.googleapis.com";

/**
 * Gemini 4 Hydrogen, which Pi 1.5.2 does not list yet. Shape follows Pi's
 * Gemini 4 Argon entry; the id was confirmed against Vertex on 2026-11-12.
 * The price is the reported list price, not yet on Google's pricing page.
 * Pi's Google transport maps only minimal, low, medium and high, so xhigh is
 * not offered. Remove this entry once the pinned Pi package lists the model:
 * Pi's own entry then takes its place.
 */
export const GEMINI_4_HYDROGEN = {
  id: "gemini-4-hydrogen",
  name: "Gemini 4 Hydrogen",
  api: "google-vertex",
  provider: "google-vertex",
  baseUrl: VERTEX_TEMPLATE_BASE_URL,
  reasoning: true,
  input: ["text", "image"],
  cost: { input: 1.5, output: 7.5, cacheRead: 0.075, cacheWrite: 0 },
  contextWindow: 1_048_576,
  maxTokens: 65_536,
  thinkingLevelMap: {
    off: null,
    minimal: null,
    low: "low",
    medium: "medium",
    high: "high",
    xhigh: null,
    max: null,
  },
  type: "chat",
} as const;

const PROJECT_ID = /^[a-z][a-z0-9-]{4,28}[a-z0-9]$/u;
const LOCATION = /^[a-z][a-z0-9-]{1,39}$/u;

export const VERTEX_PROJECT_ENV = "GOOGLE_CLOUD_PROJECT";
export const VERTEX_LOCATION_ENV = "GOOGLE_CLOUD_LOCATION";
export const VERTEX_DEFAULT_LOCATION = "global";

/**
 * A Vertex API key is bound to a service account in one project, and Vertex
 * serves a model only under that project's path; the key alone reaches
 * neither. Enrolment stores the project and location beside the key.
 */
export function vertexRouteEnv(credential: Credential): { readonly project: string; readonly location: string } {
  const env = credential.type === "api_key" ? credential.env : undefined;
  const project = env?.[VERTEX_PROJECT_ENV];
  const location = env?.[VERTEX_LOCATION_ENV] ?? VERTEX_DEFAULT_LOCATION;
  if (project === undefined || !PROJECT_ID.test(project)) {
    throw new Error(`Google Vertex AI needs the key's Google Cloud project (${VERTEX_PROJECT_ENV}) stored with it`);
  }
  if (!LOCATION.test(location)) {
    throw new Error(`Google Vertex AI location ${location} is invalid`);
  }
  return { project, location };
}

export function isValidVertexProject(value: string): boolean {
  return PROJECT_ID.test(value);
}

export function isValidVertexLocation(value: string): boolean {
  return LOCATION.test(value);
}

/**
 * The model as Pi's Vertex transport should call it: under the credential's
 * project and location. A loopback base URL, admitted only for tests, is kept.
 */
export function routedVertexModel(model: Model<Api>, credential: Credential): Model<Api> {
  if (model.baseUrl !== VERTEX_TEMPLATE_BASE_URL) {
    return model;
  }
  const { project, location } = vertexRouteEnv(credential);
  const host = location === "global" ? "aiplatform.googleapis.com" : `${location}-aiplatform.googleapis.com`;
  return { ...model, baseUrl: `https://${host}/v1/projects/${project}/locations/${location}` };
}
