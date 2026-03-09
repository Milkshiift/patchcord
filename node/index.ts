import { spawn, type ChildProcess } from 'node:child_process';
import { createInterface } from 'node:readline';

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
}

interface ResponseMessage {
  id: number;
  result?: unknown;
  error?: unknown;
}

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};

export class AudioSharePatchbay {
  #child: ChildProcess;
  #closed = false;
  #nextId = 1;
  #pending = new Map<number, PendingRequest>();
  #exitPromise: Promise<void>;

  constructor(options: AudioSharePatchbayOptions) {
    this.#child = spawn(options.command, options.args ?? [], {
      cwd: options.cwd,
      env: options.env,
      stdio: ['pipe', 'pipe', 'inherit'],
    });

    this.#exitPromise = new Promise((resolve) => {
      this.#child.once('exit', () => resolve());
      this.#child.once('error', () => resolve());
    });

    this.#child.stdin!.on('error', () => {});

    const lines = createInterface({ input: this.#child.stdout! });

    lines.on('line', (line) => {
      this.#handleLine(line);
    });

    this.#child.on('error', (error) => {
      this.#failAll(error instanceof Error ? error : new Error(String(error)));
    });

    this.#child.on('exit', (code, signal) => {
      lines.close();
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

    if (typeof message.id !== 'number') {
      return;
    }

    const pending = this.#pending.get(message.id);
    if (!pending) {
      return;
    }

    this.#pending.delete(message.id);

    if (typeof message.error === 'string') {
      pending.reject(new Error(message.error));
      return;
    }

    pending.resolve(message.result);
  }

  #failAll(error: Error): void {
    if (this.#closed) {
      return;
    }

    this.#closed = true;

    for (const pending of this.#pending.values()) {
      pending.reject(error);
    }

    this.#pending.clear();
  }

  #request<T>(method: string, payload: Record<string, unknown> = {}): Promise<T> {
    if (this.#closed) {
      return Promise.reject(new Error('patchcord is not running'));
    }

    const id = this.#nextId++;
    const line = JSON.stringify({ id, method, ...payload }) + '\n';

    return new Promise<T>((resolve, reject) => {
      this.#pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
      });

      this.#child.stdin!.write(line, 'utf8', (error) => {
        if (!error) {
          return;
        }

        const pending = this.#pending.get(id);
        if (!pending) {
          return;
        }

        this.#pending.delete(id);
        pending.reject(error instanceof Error ? error : new Error(String(error)));
      });
    });
  }

  async hasPipeWire(): Promise<boolean> {
    return this.#request<boolean>('hasPipeWire');
  }

  async listShareableNodes(includeDevices = false): Promise<ShareableNode[]> {
    return this.#request<ShareableNode[]>('listShareableNodes', { includeDevices });
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
      return;
    }

    try {
      await this.#request<null>('dispose');
    } catch (error) {
      if (!this.#closed) {
        throw error;
      }
    } finally {
      this.#closed = true;
      this.#child.stdin!.end();

      // Prevent Node.js from hanging indefinitely if the child process deadlocks
      const killTimer = setTimeout(() => {
        this.#child.kill('SIGKILL');
      }, 2000);

      killTimer.unref?.();

      await this.#exitPromise;
      clearTimeout(killTimer);
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