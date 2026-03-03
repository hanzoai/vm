import type { Subprocess } from "bun";
import type { StdioResponse } from "./types";

interface PendingRequest {
	resolve: (value: StdioResponse) => void;
	reject: (reason: Error) => void;
}

export class ShuruProcess {
	private proc: Subprocess<"pipe", "pipe", "inherit"> | null = null;
	private pending = new Map<string, PendingRequest>();
	private idCounter = 0;

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

			this.pending.set("__ready__", {
				resolve: () => {
					clearTimeout(timeout);
					resolve();
				},
				reject: (err: Error) => {
					clearTimeout(timeout);
					reject(err);
				},
			});
		});
	}

	async send(msg: Record<string, unknown>): Promise<StdioResponse> {
		if (!this.proc) throw new Error("shuru process not started");

		const id = String(++this.idCounter);
		const line = `${JSON.stringify({ id, ...msg })}\n`;
		const proc = this.proc;

		return new Promise<StdioResponse>((resolve, reject) => {
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
		let buffer = "";

		try {
			while (true) {
				const { done, value } = await reader.read();
				if (done) break;

				buffer += decoder.decode(value, { stream: true });
				const lines = buffer.split("\n");
				buffer = lines.pop() ?? "";

				for (const line of lines) {
					if (!line) continue;
					try {
						this.dispatch(JSON.parse(line) as StdioResponse);
					} catch {
						// malformed line
					}
				}
			}
		} catch {
			// stream closed
		}

		for (const [, req] of this.pending) {
			req.reject(new Error("shuru process exited unexpectedly"));
		}
		this.pending.clear();
	}

	private dispatch(msg: StdioResponse): void {
		if (msg.type === "ready") {
			const req = this.pending.get("__ready__");
			if (req) {
				this.pending.delete("__ready__");
				req.resolve(msg);
			}
			return;
		}

		const { id } = msg;
		if (!id) return;

		const req = this.pending.get(id);
		if (!req) return;
		this.pending.delete(id);

		if (msg.type === "error") {
			req.reject(new Error(msg.error));
		} else {
			req.resolve(msg);
		}
	}
}
