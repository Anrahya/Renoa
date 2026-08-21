import type { AcpTransport, AnyMessage, Stream } from '@acp-components/core';

interface AgentEventPayload {
  bridgeId: string;
  data?: string;
  message?: string;
}

function withPromptIdentity(message: AnyMessage): AnyMessage {
  const request = message as unknown as Record<string, unknown>;
  if (request.method !== 'session/prompt') return message;
  const params = request.params;
  if (!params || typeof params !== 'object' || Array.isArray(params)) return message;
  const metadataValue = (params as Record<string, unknown>)._meta;
  const metadata =
    metadataValue && typeof metadataValue === 'object' && !Array.isArray(metadataValue)
      ? (metadataValue as Record<string, unknown>)
      : {};
  const requestId = typeof metadata.requestId === 'string' ? metadata.requestId : null;
  const promptId = typeof metadata.promptId === 'string' ? metadata.promptId : null;
  if (requestId && promptId) return message;
  const turnId = requestId ?? promptId ?? globalThis.crypto.randomUUID();
  return {
    ...request,
    params: {
      ...(params as Record<string, unknown>),
      _meta: { ...metadata, requestId: turnId, promptId: turnId },
    },
  } as unknown as AnyMessage;
}

export class TauriAcpTransport implements AcpTransport {
  private closeHandlers = new Set<() => void>();
  private errorHandlers = new Set<(error: Error) => void>();
  private unlisteners: Array<() => void> = [];
  private connected = false;
  private lifecycle: Promise<void> = Promise.resolve();

  constructor(private readonly bridgeId: string) {}

  connect(): Promise<Stream> {
    const operation = this.lifecycle.then(() => this.connectNow());
    this.lifecycle = operation.then(
      () => undefined,
      () => undefined,
    );
    return operation;
  }

  private async connectNow(): Promise<Stream> {
    if (this.connected || this.unlisteners.length > 0) {
      throw new Error('Renoa ACP bridge is already connected');
    }
    const [{ invoke }, { listen }] = await Promise.all([
      import('@tauri-apps/api/core'),
      import('@tauri-apps/api/event'),
    ]);

    let controller!: ReadableStreamDefaultController<AnyMessage>;
    let readableOpen = true;
    const closeReadable = () => {
      if (!readableOpen) return;
      readableOpen = false;
      controller.close();
    };
    const failReadable = (error: Error) => {
      this.reportError(error);
      if (!readableOpen) return;
      readableOpen = false;
      controller.error(error);
    };
    const readable = new ReadableStream<AnyMessage>({
      start(nextController) {
        controller = nextController;
      },
      cancel: () => {
        this.stopAfterFailure();
      },
    });

    try {
      this.unlisteners.push(
        await listen<AgentEventPayload>('renoa-acp-output', (event) => {
          if (event.payload.bridgeId !== this.bridgeId || !event.payload.data) return;
          try {
            controller.enqueue(JSON.parse(event.payload.data) as AnyMessage);
          } catch {
            failReadable(new Error('Renoa ACP emitted an invalid JSON message'));
            this.stopAfterFailure();
          }
        }),
      );
      this.unlisteners.push(
        await listen<AgentEventPayload>('renoa-acp-closed', (event) => {
          if (event.payload.bridgeId !== this.bridgeId) return;
          this.connected = false;
          closeReadable();
          this.cleanupListeners();
          for (const handler of this.closeHandlers) handler();
        }),
      );
      this.unlisteners.push(
        await listen<AgentEventPayload>('renoa-acp-error', (event) => {
          if (event.payload.bridgeId !== this.bridgeId) return;
          failReadable(new Error(event.payload.message ?? 'Renoa ACP bridge failed'));
          this.stopAfterFailure();
        }),
      );
      this.unlisteners.push(
        await listen<AgentEventPayload>('renoa-acp-diagnostic', (event) => {
          if (event.payload.bridgeId !== this.bridgeId || !event.payload.message) return;
          globalThis.console.error(`[renoa-agent] ${event.payload.message}`);
        }),
      );
      await invoke('start_agent', { args: { bridgeId: this.bridgeId } });
      this.connected = true;
    } catch (error) {
      this.cleanupListeners();
      const message = error instanceof Error ? error.message : String(error);
      failReadable(new Error(message));
      throw error;
    }

    const writable = new WritableStream<AnyMessage>({
      write: async (message) => {
        if (!this.connected) throw new Error('Renoa ACP bridge is not connected');
        const identified = withPromptIdentity(message);
        await invoke('write_to_agent', {
          args: {
            bridgeId: this.bridgeId,
            line: `${JSON.stringify(identified)}\n`,
          },
        });
      },
      close: async () => {
        await this.scheduleStop();
      },
      abort: async () => {
        await this.scheduleStop();
      },
    });

    return { readable, writable };
  }

  disconnect(): void {
    this.stopAfterFailure();
  }

  onClose(handler: () => void): () => void {
    this.closeHandlers.add(handler);
    return () => this.closeHandlers.delete(handler);
  }

  onError(handler: (error: Error) => void): () => void {
    this.errorHandlers.add(handler);
    return () => this.errorHandlers.delete(handler);
  }

  private reportError(error: Error): void {
    for (const handler of this.errorHandlers) handler(error);
  }

  private cleanupListeners(): void {
    for (const unlisten of this.unlisteners.splice(0)) unlisten();
  }

  private scheduleStop(): Promise<void> {
    const operation = this.lifecycle.then(() => this.stop());
    this.lifecycle = operation.then(
      () => undefined,
      () => undefined,
    );
    return operation;
  }

  private stopAfterFailure(): void {
    void this.scheduleStop().catch((error: unknown) => {
      this.reportError(error instanceof Error ? error : new Error(String(error)));
    });
  }

  private async stop(): Promise<void> {
    this.cleanupListeners();
    if (!this.connected) return;
    this.connected = false;
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('kill_agent', { args: { bridgeId: this.bridgeId } });
  }
}
