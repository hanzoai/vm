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

export type StdioResponse =
	| { type: "ready" }
	| {
			type: "exec";
			id: string;
			stdout: string;
			stderr: string;
			exit_code: number;
	  }
	| { type: "checkpoint"; id: string; ok: boolean }
	| { type: "error"; id: string; error: string };
