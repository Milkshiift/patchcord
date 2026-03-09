import { spawn, type ChildProcess } from 'node:child_process';
import { createInterface, type Interface } from 'node:readline';

export interface ShareableNode {
  id: number;
  displayName: string;
  applicationName: string | null;
  nodeName: string | null;
  description: string | null;
  mediaName: string | null;
  binary: string | null;
  processId: number | null;
  isDevice: boolean;
}

export interface VirtualSinkInfo {
  sinkName: string;
  monitorSource: string;
  nodeId: number;
}

export interface AudioSharePatchbayOptions {
  command: string;
  args?: readonly string[];
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  requestTimeoutMs?: number;
  shutdownTimeoutMs?: number;
}

interface ResponseMessage {
  id: unknown;
  result?: unknown;
  error?: unknown;
}

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
};

export class AudioSharePatchbay {
  #child: ChildProcess;
  #lines: Interface;
  #closed = false;
  #closing = false;
  #nextId = 1;
  #pending = new Map<number, PendingRequest>();
  #exitPromise: Promise<void>;
  #requestTimeoutMs: number;
  #shutdownTimeoutMs: number;

  constructor(options: AudioSharePatchbayOptions) {
    this.#requestTimeoutMs = sanitizeTimeout(options.requestTimeoutMs, 15_000);
    this.#shutdownTimeoutMs = sanitizeTimeout(options.shutdownTimeoutMs, 2_000);

    this.#child = spawn(options.command, options.args ?? [], {
      cwd: options.cwd,
      env: options.env,
      stdio: ['pipe', 'pipe', 'inherit'],
    });

    const stdin = this.#child.stdin;
    const stdout = this.#child.stdout;

    if (!stdin || !stdout) {
      this.#closed = true;
      this.#closing = true;
      throw new Error('patchcord failed to start with piped stdin/stdout');
    }

    this.#lines = createInterface({
      input: stdout,
      crlfDelay: Infinity,
    });

    this.#exitPromise = new Promise((resolve) => {
      this.#child.once('exit', () => resolve());
      this.#child.once('error', () => resolve());
    });

    stdin.on('error', () => {});

    this.#lines.on('line', (line) => {
      this.#handleLine(line);
    });

    this.#child.on('error', (error) => {
      this.#abortProcess(normalizeError(error));
    });

    this.#child.on('exit', (code, signal) => {
      this.#lines.close();
      this.#failAll(
          new Error(`patchcord exited (${signal ?? code ?? 'unknown'})`),
      );
    });
  }

  #handleLine(line: string): void {
    let message: ResponseMessage;

    try {
      message = JSON.parse(line) as ResponseMessage;
    } catch {
      return;
    }

    if (!Number.isSafeInteger(message.id) || (message.id as number) < 0) {
      return;
    }

    const id = message.id as number;
    const pending = this.#pending.get(id);

    if (!pending) {
      return;
    }

    this.#pending.delete(id);
    clearTimeout(pending.timer);

    const errorMessage = normalizeRemoteError(message.error);

    if (errorMessage !== null) {
      pending.reject(new Error(errorMessage));
      return;
    }

    pending.resolve(message.result);
  }

  #failAll(error: Error): void {
    this.#closed = true;
    this.#closing = true;

    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }

    this.#pending.clear();
  }

  #abortProcess(error: Error): void {
    this.#failAll(error);

    try {
      this.#child.kill('SIGKILL');
    } catch {
      // ignore
    }
  }

  #sendRequest<T>(
      method: string,
      payload: Record<string, unknown> = {},
      allowWhenClosing = false,
  ): Promise<T> {
    if (this.#closed) {
      return Promise.reject(new Error('patchcord is not running'));
    }

    if (this.#closing && !allowWhenClosing) {
      return Promise.reject(new Error('patchcord is shutting down'));
    }

    const stdin = this.#child.stdin;

    if (!stdin || stdin.destroyed || stdin.writableEnded) {
      return Promise.reject(new Error('patchcord is not running'));
    }

    const id = this.#nextId++;
    const line = JSON.stringify({ id, method, ...payload }) + '\n';

    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        const pending = this.#pending.get(id);

        if (!pending) {
          return;
        }

        this.#pending.delete(id);

        const error = new Error(
            `patchcord request timed out after ${this.#requestTimeoutMs}ms (${method})`,
        );

        pending.reject(error);
        this.#abortProcess(error);
      }, this.#requestTimeoutMs);

      timer.unref?.();

      this.#pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
        timer,
      });

      stdin.write(line, 'utf8', (error) => {
        if (!error) {
          return;
        }

        const pending = this.#pending.get(id);

        if (!pending) {
          return;
        }

        this.#pending.delete(id);
        clearTimeout(pending.timer);
        pending.reject(normalizeError(error));
      });
    });
  }

  #request<T>(method: string, payload: Record<string, unknown> = {}): Promise<T> {
    return this.#sendRequest<T>(method, payload, false);
  }

  async hasPipeWire(): Promise<boolean> {
    return this.#request<boolean>('hasPipeWire');
  }

  async listShareableNodes(includeDevices = false): Promise<ShareableNode[]> {
    return this.#request<ShareableNode[]>('listShareableNodes', {
      includeDevices,
    });
  }

  async ensureVirtualSink(): Promise<VirtualSinkInfo> {
    return this.#request<VirtualSinkInfo>('ensureVirtualSink');
  }

  async routeNodes(nodeIds: number[]): Promise<VirtualSinkInfo> {
    return this.#request<VirtualSinkInfo>('routeNodes', { nodeIds });
  }

  async clearRoutes(): Promise<void> {
    await this.#request<null>('clearRoutes');
  }

  async dispose(): Promise<void> {
    if (this.#closed) {
      await this.#exitPromise.catch(() => {});
      return;
    }

    if (this.#closing) {
      await this.#exitPromise.catch(() => {});
      return;
    }

    this.#closing = true;

    const shutdownTimer = setTimeout(() => {
      this.#abortProcess(
          new Error(
              `patchcord did not shut down within ${this.#shutdownTimeoutMs}ms`,
          ),
      );
    }, this.#shutdownTimeoutMs);

    shutdownTimer.unref?.();

    let requestError: unknown;

    try {
      await this.#sendRequest<null>('dispose', {}, true);
    } catch (error) {
      requestError = error;
    } finally {
      this.#child.stdin?.end();

      try {
        await this.#exitPromise;
      } finally {
        clearTimeout(shutdownTimer);
        this.#closed = true;
        this.#closing = true;
      }
    }

    if (requestError && !this.#closed) {
      throw normalizeError(requestError);
    }

    if (
        requestError &&
        requestError instanceof Error &&
        !requestError.message.startsWith('patchcord exited (')
    ) {
      throw requestError;
    }
  }
}

export async function hasPipeWire(
    options: AudioSharePatchbayOptions,
): Promise<boolean> {
  const patchbay = new AudioSharePatchbay(options);

  try {
    return await patchbay.hasPipeWire();
  } finally {
    try {
      await patchbay.dispose();
    } catch {
      // ignore shutdown errors
    }
  }
}

function sanitizeTimeout(value: number | undefined, fallback: number): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
    return fallback;
  }

  return Math.floor(value);
}

function normalizeError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
}

function normalizeRemoteError(value: unknown): string | null {
  if (value == null) {
    return null;
  }

  return typeof value === 'string' ? value : String(value);
}