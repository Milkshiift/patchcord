import { EventEmitter } from "node:events";

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
    virtualMicName?: string | null;
    virtualMicDescription?: string | null;
}

export interface AudioSharePatchbayOptions {
    command: string;
    args?: readonly string[];
    cwd?: string;
    env?: NodeJS.ProcessEnv;
    requestTimeoutMs?: number;
    shutdownTimeoutMs?: number;
    sinkPrefix?: string;
    sinkDescription?: string;
    virtualMic?: boolean;
    virtualMicName?: string;
    virtualMicDescription?: string;
}

export declare class AudioSharePatchbay extends EventEmitter {
    constructor(options: AudioSharePatchbayOptions);
    hasPipeWire(): Promise<boolean>;
    listShareableNodes(includeDevices?: boolean): Promise<ShareableNode[]>;
    ensureVirtualSink(): Promise<VirtualSinkInfo>;
    routeNodes(nodeIds: number[]): Promise<VirtualSinkInfo>;
    clearRoutes(): Promise<void>;
    dispose(): Promise<void>;

    on(eventName: 'graphChanged' | 'monitorDied', listener: () => void): this;
    once(eventName: 'graphChanged' | 'monitorDied', listener: () => void): this;
    off(eventName: 'graphChanged' | 'monitorDied', listener: () => void): this;
    emit(eventName: 'graphChanged' | 'monitorDied'): boolean;

    on(eventName: string | symbol, listener: (...args: any[]) => void): this;
    once(eventName: string | symbol, listener: (...args: any[]) => void): this;
    off(eventName: string | symbol, listener: (...args: any[]) => void): this;
    emit(eventName: string | symbol, ...args: any[]): boolean;
}

export declare function hasPipeWire(
    options: AudioSharePatchbayOptions,
): Promise<boolean>;