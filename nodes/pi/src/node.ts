import { PiHarness } from "./harness.js";
import type { DeviceCredentials } from "./protocol.js";
import { uuid } from "./protocol.js";
import { Pulse } from "./pulse.js";
import { reconnect } from "./session.js";
import { NodeState } from "./state.js";

export interface PiNodeOptions {
  readonly endpoint: string;
  readonly credentials: DeviceCredentials;
  readonly statePath: string;
  readonly harness: PiHarness;
}

export class PiNode {
  readonly #endpoint: string;
  readonly #credentials: DeviceCredentials;
  readonly #harness: PiHarness;
  readonly #pulse = new Pulse();
  readonly #state: NodeState;
  #started = false;

  constructor(options: PiNodeOptions) {
    const endpoint = new URL(options.endpoint);
    if (endpoint.protocol !== "ws:" && endpoint.protocol !== "wss:") {
      throw new Error("RCP endpoint must use ws or wss");
    }
    uuid(options.credentials.deviceId, "credentials.deviceId");
    if (options.credentials.credential === "") {
      throw new Error("credentials.credential must not be empty");
    }
    if (options.statePath === "") {
      throw new Error("statePath must not be empty");
    }
    this.#endpoint = endpoint.toString();
    this.#credentials = { ...options.credentials };
    this.#harness = options.harness;
    this.#state = new NodeState(options.statePath, () => this.#pulse.wake());
  }

  async run(signal: AbortSignal): Promise<void> {
    if (this.#started) {
      throw new Error("Pi node can only run once");
    }
    this.#started = true;
    this.#state.recoverInterrupted();
    const runtime = new AbortController();
    const stop = () => runtime.abort(signal.reason);
    signal.addEventListener("abort", stop, { once: true });
    if (signal.aborted) {
      stop();
    }
    const worker = this.#work(runtime.signal);
    const connection = reconnect(
      this.#endpoint,
      this.#credentials,
      this.#state,
      this.#pulse,
      runtime.signal,
    );
    try {
      await Promise.all([worker, connection]);
    } catch (error) {
      runtime.abort(error);
      await Promise.allSettled([worker, connection]);
      throw error;
    } finally {
      runtime.abort();
      signal.removeEventListener("abort", stop);
      this.#state.close();
    }
  }

  async #work(signal: AbortSignal): Promise<void> {
    while (!signal.aborted) {
      const observedGeneration = this.#pulse.generation;
      const execution = this.#state.claimNext();
      if (execution === null) {
        try {
          await this.#pulse.wait(observedGeneration, signal);
        } catch {
          return;
        }
        continue;
      }
      try {
        await this.#harness.execute(execution, this.#state, signal);
      } catch (error) {
        this.#state.finish(
          execution.commandId,
          { status: "failed", error: asError(error).message },
          this.#state.loadMessages(execution.taskId),
        );
      }
    }
  }
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}
