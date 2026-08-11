export class Pulse {
  readonly #listeners = new Set<() => void>();
  #generation = 0;

  get generation(): number {
    return this.#generation;
  }

  wake(): void {
    this.#generation += 1;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  wait(observedGeneration: number, signal: AbortSignal): Promise<void> {
    if (signal.aborted) {
      return Promise.reject(signal.reason);
    }
    if (observedGeneration !== this.#generation) {
      return Promise.resolve();
    }
    return new Promise((resolve, reject) => {
      const complete = () => {
        cleanup();
        resolve();
      };
      const abort = () => {
        cleanup();
        reject(signal.reason);
      };
      const cleanup = () => {
        this.#listeners.delete(complete);
        signal.removeEventListener("abort", abort);
      };
      this.#listeners.add(complete);
      signal.addEventListener("abort", abort, { once: true });
    });
  }
}
