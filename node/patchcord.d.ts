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
    command?: string;
    args?: readonly string[];
    cwd?: string;
    env?: NodeJS.ProcessEnv;
    requestTimeoutMs?: number;
    shutdownTimeoutMs?: number;
}

export declare class AudioSharePatchbay {
    constructor(options: AudioSharePatchbayOptions);
    hasPipeWire(): Promise<boolean>;
    listShareableNodes(includeDevices?: boolean): Promise<ShareableNode[]>;
    ensureVirtualSink(): Promise<VirtualSinkInfo>;
    routeNodes(nodeIds: number[]): Promise<VirtualSinkInfo>;
    clearRoutes(): Promise<void>;
    dispose(): Promise<void>;
}

export declare function hasPipeWire(
    options: AudioSharePatchbayOptions,
): Promise<boolean>;