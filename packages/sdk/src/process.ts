import type { Subprocess } from "bun";
import type { JsonRpcResponse, JsonRpcResult } from "./types";

interface PendingRequest {
	resolve: (value: JsonRpcResult) => void;
	reject: (reason: Error) => void;
}

export class ShuruProcess {
	private proc: Subprocess<"pipe", "pipe", "inherit"> | null = null;
	private pending = new Map<number, PendingRequest>();
	private idCounter = 0;
	private onReady: (() => void) | null = null;
	private onReadyError: ((err: Error) => void) | null = null;

	async start(args: string[]): Promise<void> {
		this.proc = Bun.spawn(args, {
			stdin: "pipe",
			stdout: "pipe",
			stderr: "inherit",
		});

		this.readLoop();

		await new Promise<void>((resolve, reject) => {
			const timeout = setTimeout(() => {
				reject(new Error("shuru: timed out waiting for ready signal (30s)"));
			}, 30_000);

			this.onReady = () => {
				clearTimeout(timeout);
				resolve();
			};
			this.onReadyError = (err: Error) => {
				clearTimeout(timeout);
				reject(err);
			};
		});
	}

	send(
		method: string,
		params: Record<string, unknown>,
	): Promise<JsonRpcResult> {
		if (!this.proc) throw new Error("shuru process not started");

		const id = ++this.idCounter;
		const line = `${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`;
		const proc = this.proc;

		return new Promise<JsonRpcResult>((resolve, reject) => {
			this.pending.set(id, { resolve, reject });
			proc.stdin.write(line);
			proc.stdin.flush();
		});
	}

	async stop(): Promise<void> {
		if (!this.proc) return;

		try {
			this.proc.stdin.end();
		} catch {
			// stdin may already be closed
		}

		const exited = this.proc.exited;
		const timeout = new Promise<never>((_, reject) =>
			setTimeout(
				() => reject(new Error("shuru: shutdown timed out (5s)")),
				5_000,
			),
		);

		try {
			await Promise.race([exited, timeout]);
		} catch {
			this.proc.kill();
			await this.proc.exited;
		}

		for (const [, req] of this.pending) {
			req.reject(new Error("shuru process stopped"));
		}
		this.pending.clear();
		this.proc = null;
	}

	private async readLoop(): Promise<void> {
		if (!this.proc?.stdout) return;

		const reader = this.proc.stdout.getReader();
		const decoder = new TextDecoder();
		let remainder = "";

		try {
			while (true) {
				const { done, value } = await reader.read();
				if (done) break;

				remainder += decoder.decode(value, { stream: true });

				while (true) {
					const newlineIdx = remainder.indexOf("\n");
					if (newlineIdx === -1) break;
					const line = remainder.slice(0, newlineIdx);
					remainder = remainder.slice(newlineIdx + 1);

					if (!line) continue;

					try {
						const msg = JSON.parse(line) as JsonRpcResponse;
						this.dispatch(msg);
					} catch {
						// malformed line
					}
				}
			}
		} catch {
			// stream closed
		}

		if (this.onReadyError) {
			this.onReadyError(new Error("shuru process exited unexpectedly"));
			this.onReady = null;
			this.onReadyError = null;
		}
		for (const [, req] of this.pending) {
			req.reject(new Error("shuru process exited unexpectedly"));
		}
		this.pending.clear();
	}

	private dispatch(msg: JsonRpcResponse): void {
		if ("method" in msg && msg.method === "ready") {
			if (this.onReady) {
				this.onReady();
				this.onReady = null;
				this.onReadyError = null;
			}
			return;
		}

		if (!("id" in msg) || msg.id == null) return;
		const id = msg.id as number;

		const req = this.pending.get(id);
		if (!req) return;
		this.pending.delete(id);

		if ("error" in msg) {
			req.reject(new Error(msg.error.message));
		} else {
			req.resolve(msg as JsonRpcResult);
		}
	}
}
