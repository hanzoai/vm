import { describe, expect, test } from "bun:test";
import { buildArgs } from "../../src/sandbox";

describe("buildArgs", () => {
	test("minimal options", () => {
		const args = buildArgs("hanzo-vm", {});
		expect(args).toEqual(["hanzo-vm", "run", "--stdio"]);
	});

	test("custom binary path", () => {
		const args = buildArgs("/usr/local/bin/hanzo-vm", {});
		expect(args).toEqual(["/usr/local/bin/hanzo-vm", "run", "--stdio"]);
	});

	test("multi-word binary", () => {
		const args = buildArgs("bun mock-vm.ts", {});
		expect(args).toEqual(["bun", "mock-vm.ts", "run", "--stdio"]);
	});

	test("from checkpoint", () => {
		const args = buildArgs("hanzo-vm", { from: "my-checkpoint" });
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"--from",
			"my-checkpoint",
		]);
	});

	test("cpus and memory", () => {
		const args = buildArgs("hanzo-vm", { cpus: 4, memory: 4096 });
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"--cpus",
			"4",
			"--memory",
			"4096",
		]);
	});

	test("disk size", () => {
		const args = buildArgs("hanzo-vm", { diskSize: 8192 });
		expect(args).toEqual(["hanzo-vm", "run", "--stdio", "--disk-size", "8192"]);
	});

	test("allow net", () => {
		const args = buildArgs("hanzo-vm", { allowNet: true });
		expect(args).toEqual(["hanzo-vm", "run", "--stdio", "--allow-net"]);
	});

	test("allowNet false is omitted", () => {
		const args = buildArgs("hanzo-vm", { allowNet: false });
		expect(args).toEqual(["hanzo-vm", "run", "--stdio"]);
	});

	test("allow host writes", () => {
		const args = buildArgs("hanzo-vm", { allowHostWrites: true });
		expect(args).toEqual(["hanzo-vm", "run", "--stdio", "--allow-host-writes"]);
	});

	test("allowHostWrites false is omitted", () => {
		const args = buildArgs("hanzo-vm", { allowHostWrites: false });
		expect(args).toEqual(["hanzo-vm", "run", "--stdio"]);
	});

	test("port forwards", () => {
		const args = buildArgs("hanzo-vm", { ports: ["8080:80", "3000:3000"] });
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"-p",
			"8080:80",
			"-p",
			"3000:3000",
		]);
	});

	test("mounts", () => {
		const args = buildArgs("hanzo-vm", {
			mounts: { "./src": "/workspace", "./data": "/data" },
		});
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"--mount",
			"./src:/workspace",
			"--mount",
			"./data:/data",
		]);
	});

	test("rw mount with allow host writes", () => {
		const args = buildArgs("hanzo-vm", {
			allowHostWrites: true,
			mounts: { "./src": "/workspace:rw" },
		});
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"--allow-host-writes",
			"--mount",
			"./src:/workspace:rw",
		]);
	});

	test("secrets", () => {
		const args = buildArgs("hanzo-vm", {
			allowNet: true,
			secrets: {
				API_KEY: { from: "OPENAI_API_KEY", hosts: ["api.openai.com"] },
			},
		});
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"--allow-net",
			"--secret",
			"API_KEY=OPENAI_API_KEY@api.openai.com",
		]);
	});

	test("secrets with multiple hosts", () => {
		const args = buildArgs("hanzo-vm", {
			secrets: {
				TOKEN: { from: "MY_TOKEN", hosts: ["a.com", "b.com"] },
			},
		});
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"--secret",
			"TOKEN=MY_TOKEN@a.com,b.com",
		]);
	});

	test("network allow hosts", () => {
		const args = buildArgs("hanzo-vm", {
			network: { allow: ["api.openai.com", "*.npmjs.org"] },
		});
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"--allow-host",
			"api.openai.com",
			"--allow-host",
			"*.npmjs.org",
		]);
	});

	test("all options combined", () => {
		const args = buildArgs("hanzo-vm", {
			from: "base",
			cpus: 2,
			memory: 2048,
			diskSize: 4096,
			allowNet: true,
			ports: ["8080:80"],
			mounts: { "./src": "/workspace" },
		});
		expect(args).toEqual([
			"hanzo-vm",
			"run",
			"--stdio",
			"--from",
			"base",
			"--cpus",
			"2",
			"--memory",
			"2048",
			"--disk-size",
			"4096",
			"--allow-net",
			"-p",
			"8080:80",
			"--mount",
			"./src:/workspace",
		]);
	});
});
