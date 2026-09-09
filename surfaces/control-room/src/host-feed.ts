import { parseHost, type HostSnapshot } from "./host-contract";

export type FeedState = {
  status: "connecting" | "connected" | "reconnecting" | "locked" | "forbidden";
  snapshot: HostSnapshot | null;
  receivedAt: number | null;
  error: string | null;
};
export const initialFeed: FeedState = { status: "connecting", snapshot: null, receivedAt: null, error: null };

/** A network connection can fail without invalidating the browser's durable login. */
export class HostFeed {
  private state: FeedState = initialFeed;
  private stopped = true;
  private failures = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private request: AbortController | undefined;
  private pending = false;
  private generation = 0;

  constructor(private readonly changed: (state: FeedState) => void,
    private readonly transport: typeof fetch = (input, init) => fetch(input, init)) {}

  start(): void { this.stopped = false; this.refresh(); }
  stop(): void {
    this.stopped = true;
    clearTimeout(this.timer);
    this.request?.abort();
  }
  lock(): void {
    this.generation += 1;
    clearTimeout(this.timer);
    this.pending = false;
    this.request?.abort();
    this.publish({ status: "locked", snapshot: null, receivedAt: null, error: null });
  }
  refresh(): void {
    if (this.stopped) return;
    clearTimeout(this.timer);
    if (this.request) { this.pending = true; return; }
    void this.read();
  }
  private publish(next: FeedState): void { this.state = next; this.changed(next); }

  private async read(): Promise<void> {
    const generation = this.generation;
    const request = new AbortController();
    this.request = request;
    const timeout = setTimeout(() => request.abort(), 20_000);
    let retry = true;
    try {
      const response = await this.transport("/v1/host", {
        credentials: "same-origin", cache: "no-store", signal: request.signal,
      });
      if (this.stopped || generation !== this.generation) return;
      if (response.status === 401 || response.status === 403) {
        this.publish({ status: response.status === 401 ? "locked" : "forbidden", snapshot: null, receivedAt: null,
          error: response.status === 403 ? "This browser is signed in as a different Host owner." : null });
        retry = false;
        return;
      }
      if (!response.ok) throw new Error("The Host is temporarily unavailable. Reconnecting automatically.");
      const snapshot = parseHost(await response.json());
      if (this.stopped || generation !== this.generation) return;
      this.failures = 0;
      this.publish({ status: "connected", snapshot, receivedAt: Date.now(), error: null });
    } catch (error) {
      if (this.stopped || generation !== this.generation) return;
      this.failures += 1;
      this.publish({ ...this.state, status: "reconnecting", error: error instanceof Error && error.name !== "AbortError"
        ? error.message : "The connection was interrupted. Reconnecting automatically." });
    } finally {
      clearTimeout(timeout);
      this.request = undefined;
      if (!this.stopped && this.pending) { this.pending = false; this.refresh(); }
      else if (!this.stopped && retry && generation === this.generation) {
        const delay = this.failures ? Math.min(30_000, 1000 * 2 ** Math.min(this.failures, 5)) * (0.8 + Math.random() * 0.4)
          : document.visibilityState === "hidden" ? 60_000 : 5_000;
        this.timer = setTimeout(() => this.refresh(), delay);
      }
    }
  }
}
