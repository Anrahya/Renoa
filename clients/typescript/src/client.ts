import {
  RcpSurfaceClientCore,
  assertRcpConnectionOptions,
} from "./core-client.js";
import type { RcpSurfaceClientOptions } from "./protocol.js";
import { SurfaceState } from "./state.js";

export { RcpError } from "./core-client.js";
export type { ApplyTaskEvent, CommandSubmission } from "./core-client.js";

export class RcpSurfaceClient extends RcpSurfaceClientCore {
  constructor(options: RcpSurfaceClientOptions) {
    assertRcpConnectionOptions(options.endpoint, options.authentication);
    if (typeof options.statePath !== "string" || options.statePath === "") {
      throw new Error("statePath must be a non-empty string");
    }
    super({
      endpoint: options.endpoint,
      authentication: options.authentication,
      state: new SurfaceState(options.statePath),
    });
  }
}
