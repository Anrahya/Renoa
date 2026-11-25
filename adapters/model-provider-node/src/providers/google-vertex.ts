import type { Api, Model } from "@earendil-works/pi-ai";

import type { Credential } from "../credentials.js";

/**
 * The endpoint template of every Vertex entry in Pi's catalog
 * (`packages/ai/src/providers/google-vertex.ts`, v1.6.1). It is the binding's
 * base URL; the route a request takes comes from the stored credential.
 */
export const VERTEX_TEMPLATE_BASE_URL = "https://{location}-aiplatform.googleapis.com";

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
