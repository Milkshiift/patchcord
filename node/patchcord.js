import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { EventEmitter } from 'node:events';

export class AudioSharePatchbay extends EventEmitter {
    #child;
    #lines;
    #closed = false;
    #closing = false;
    #nextId = 1;
    #pending = new Map();
    #exitPromise;
    #requestTimeoutMs;
    #shutdownTimeoutMs;

    constructor(options) {
        super(); // Initialize the EventEmitter

        this.#requestTimeoutMs = sanitizeTimeout(options.requestTimeoutMs, 15_000);
        this.#shutdownTimeoutMs = sanitizeTimeout(options.shutdownTimeoutMs, 2_000);

        const args = [...(options.args ?? [])];

        if (typeof options.sinkPrefix === 'string') {
            args.push('--sink-prefix', options.sinkPrefix);
        }

        if (typeof options.sinkDescription === 'string') {
            args.push('--sink-description', options.sinkDescription);
        }

        if (options.virtualMic) {
            args.push('--virtual-mic');
        }

        if (typeof options.virtualMicName === 'string') {
            args.push('--virtual-mic-name', options.virtualMicName);
        }

        if (typeof options.virtualMicDescription === 'string') {
            args.push('--virtual-mic-description', options.virtualMicDescription);
        }

        this.#child = spawn(options.command, args, {
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

    #handleLine(line) {
        let message;

        try {
            message = JSON.parse(line);
        } catch {
            return;
        }

        if (typeof message.event === 'string') {
            this.emit(message.event, message.data);
            return;
        }

        if (!Number.isSafeInteger(message.id) || message.id < 0) {
            return;
        }

        const id = message.id;
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

    #failAll(error) {
        this.#closed = true;
        this.#closing = true;

        for (const pending of this.#pending.values()) {
            clearTimeout(pending.timer);
            pending.reject(error);
        }

        this.#pending.clear();
    }

    #abortProcess(error) {
        this.#failAll(error);

        try {
            this.#child.kill('SIGKILL');
        } catch {
            // ignore
        }
    }

    #sendRequest(method, payload = {}, allowWhenClosing = false) {
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

        return new Promise((resolve, reject) => {
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

            this.#pending.set(id, { resolve, reject, timer });

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

    #request(method, payload = {}) {
        return this.#sendRequest(method, payload, false);
    }

    async hasPipeWire() {
        return this.#request('hasPipeWire');
    }

    async listShareableNodes(includeDevices = false) {
        return this.#request('listShareableNodes', { includeDevices });
    }

    async ensureVirtualSink() {
        return this.#request('ensureVirtualSink');
    }

    async routeNodes(nodeIds) {
        return this.#request('routeNodes', { nodeIds });
    }

    async clearRoutes() {
        await this.#request('clearRoutes');
    }

    async dispose() {
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

        let requestError;

        try {
            await this.#sendRequest('dispose', {}, true);
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
            requestError instanceof Error &&
            !requestError.message.startsWith('patchcord exited (')
        ) {
            throw requestError;
        }
    }
}

export async function hasPipeWire(options) {
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

function sanitizeTimeout(value, fallback) {
    if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
        return fallback;
    }

    return Math.floor(value);
}

function normalizeError(value) {
    return value instanceof Error ? value : new Error(String(value));
}

function normalizeRemoteError(value) {
    if (value == null) {
        return null;
    }

    return typeof value === 'string' ? value : String(value);
}