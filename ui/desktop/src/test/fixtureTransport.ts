import type { AcpTransport, AnyMessage, Stream } from '@acp-components/core';

type JsonRpcMessage = Record<string, unknown> & {
  id?: number | string;
  method?: string;
  params?: Record<string, unknown>;
};

const SESSION_ID = '11111111-1111-4111-8111-111111111111';

function selector(
  id: string,
  name: string,
  category: string,
  currentValue: string,
  options: Array<{ name: string; value: string }>,
) {
  return {
    id,
    name,
    category,
    type: 'select',
    currentValue,
    options,
  };
}

export class FixtureAcpTransport implements AcpTransport {
  readonly requests: string[] = [];
  readonly sessionId = SESSION_ID;
  private controller: ReadableStreamDefaultController<AnyMessage> | null = null;
  private closeHandlers = new Set<() => void>();
  private errorHandlers = new Set<(error: Error) => void>();
  private model = 'fixture-precise';
  private reasoning = 'high';
  private pendingPrompt: { id: number | string; sessionId: string } | null = null;

  async connect(): Promise<Stream> {
    const readable = new ReadableStream<AnyMessage>({
      start: (controller) => {
        this.controller = controller;
      },
    });
    const writable = new WritableStream<AnyMessage>({
      write: (message) => this.receive(message as JsonRpcMessage),
      close: () => this.close(),
      abort: () => this.close(),
    });
    return { readable, writable };
  }

  disconnect(): void {
    this.close();
  }

  onClose(handler: () => void): () => void {
    this.closeHandlers.add(handler);
    return () => this.closeHandlers.delete(handler);
  }

  onError(handler: (error: Error) => void): () => void {
    this.errorHandlers.add(handler);
    return () => this.errorHandlers.delete(handler);
  }

  private receive(message: JsonRpcMessage): void {
    const method = message.method;
    if (!method) return;
    this.requests.push(method);
    try {
      switch (method) {
        case 'initialize':
          this.respond(message.id, {
            protocolVersion: 1,
            agentCapabilities: {
              loadSession: true,
              promptCapabilities: { image: true },
            },
            agentInfo: { name: 'renoa-fixture', version: '0.0.0' },
            authMethods: [],
          });
          break;
        case 'session/new':
          this.respond(message.id, {
            sessionId: SESSION_ID,
            configOptions: this.configOptions(),
          });
          break;
        case 'session/load':
          this.notify(SESSION_ID, {
            sessionUpdate: 'user_message_chunk',
            content: { type: 'text', text: 'Durable question' },
            messageId: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
          });
          this.notify(SESSION_ID, {
            sessionUpdate: 'agent_message_chunk',
            content: { type: 'text', text: 'Durable answer' },
            messageId: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
          });
          this.respond(message.id, { configOptions: this.configOptions() });
          break;
        case 'session/set_config_option':
          this.configure(message.params);
          this.respond(message.id, { configOptions: this.configOptions() });
          break;
        case 'session/prompt':
          this.prompt(message);
          break;
        case 'session/cancel':
          this.cancel();
          break;
        default:
          this.fail(message.id, -32601, `Unsupported fixture method: ${method}`);
      }
    } catch (error) {
      const failure = error instanceof Error ? error : new Error(String(error));
      for (const handler of this.errorHandlers) handler(failure);
      this.fail(message.id, -32603, failure.message);
    }
  }

  private prompt(message: JsonRpcMessage): void {
    const sessionId = String(message.params?.sessionId ?? SESSION_ID);
    const prompt = message.params?.prompt;
    const first = Array.isArray(prompt) ? prompt[0] as Record<string, unknown> | undefined : undefined;
    const text = first?.type === 'text' ? String(first.text ?? '') : '';
    const id = message.id;
    if (id === undefined) return;

    this.notify(sessionId, {
      sessionUpdate: 'agent_thought_chunk',
      content: { type: 'text', text: 'Inspecting the workspace before answering.' },
      messageId: 'fixture-thought',
    });

    if (text.toLowerCase().includes('wait')) {
      this.pendingPrompt = { id, sessionId };
      return;
    }

    this.notify(sessionId, {
      sessionUpdate: 'agent_message_chunk',
      content: { type: 'text', text: 'I checked the workspace.' },
      messageId: 'fixture-answer',
    });
    this.notify(sessionId, {
      sessionUpdate: 'tool_call',
      toolCallId: 'fixture-read',
      title: 'Read workspace file',
      kind: 'read',
      status: 'in_progress',
      content: [],
      locations: [],
      rawInput: { path: 'value.txt' },
      rawOutput: null,
    });
    this.notify(sessionId, {
      sessionUpdate: 'tool_call_update',
      toolCallId: 'fixture-read',
      status: 'completed',
      content: [{ type: 'content', content: { type: 'text', text: 'value\n' } }],
      rawOutput: { text: 'value\n' },
    });
    this.notify(sessionId, {
      sessionUpdate: 'agent_message_chunk',
      content: { type: 'text', text: 'The requested file is ready.' },
      messageId: 'fixture-final',
    });
    this.respond(id, { stopReason: 'end_turn' });
  }

  private cancel(): void {
    const pending = this.pendingPrompt;
    if (!pending) return;
    this.pendingPrompt = null;
    this.respond(pending.id, { stopReason: 'cancelled' });
  }

  private configure(params: Record<string, unknown> | undefined): void {
    const configId = String(params?.configId ?? '');
    const value = String(params?.value ?? '');
    if (configId === 'model') this.model = value;
    if (configId === 'thought_level') this.reasoning = value;
  }

  private configOptions() {
    return [
      selector('model', 'Model', 'model', this.model, [
        { name: 'Fixture Precise', value: 'fixture-precise' },
        { name: 'Fixture Fast', value: 'fixture-fast' },
      ]),
      selector('thought_level', 'Reasoning', 'thought_level', this.reasoning, [
        { name: 'Low', value: 'low' },
        { name: 'High', value: 'high' },
      ]),
    ];
  }

  private notify(sessionId: string, update: Record<string, unknown>): void {
    this.enqueue({
      jsonrpc: '2.0',
      method: 'session/update',
      params: { sessionId, update },
    });
  }

  private respond(id: JsonRpcMessage['id'], result: unknown): void {
    if (id === undefined) return;
    this.enqueue({ jsonrpc: '2.0', id, result });
  }

  private fail(id: JsonRpcMessage['id'], code: number, message: string): void {
    if (id === undefined) return;
    this.enqueue({ jsonrpc: '2.0', id, error: { code, message } });
  }

  private enqueue(message: JsonRpcMessage): void {
    this.controller?.enqueue(message as AnyMessage);
  }

  private close(): void {
    if (!this.controller) return;
    this.controller.close();
    this.controller = null;
    for (const handler of this.closeHandlers) handler();
  }
}
