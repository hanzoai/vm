export interface StartOptions {
	from?: string;
	cpus?: number;
	memory?: number;
	diskSize?: number;
	allowNet?: boolean;
	ports?: string[];
	mounts?: Record<string, string>;
	shuruBin?: string;
}

export interface ExecResult {
	stdout: string;
	stderr: string;
	exitCode: number;
}

// --- JSON-RPC 2.0 wire types (internal) ---

export interface JsonRpcResult {
	jsonrpc: "2.0";
	id: number;
	result: unknown;
}

export interface JsonRpcError {
	jsonrpc: "2.0";
	id: number;
	error: { code: number; message: string };
}

export interface JsonRpcNotification {
	jsonrpc: "2.0";
	method: string;
}

export type JsonRpcResponse =
	| JsonRpcResult
	| JsonRpcError
	| JsonRpcNotification;
