# @superhq/shuru

TypeScript SDK for [shuru](https://github.com/superhq-ai/shuru) — programmatic access to ephemeral Linux microVMs on macOS.

## Install

```sh
bun add @superhq/shuru
```

## Usage

```ts
import { Sandbox } from "@superhq/shuru";

const sb = await Sandbox.start();

const result = await sb.exec("echo hello");
console.log(result.stdout); // "hello\n"

await sb.writeFile("/tmp/app.ts", "console.log('hi')");
const data = await sb.readFile("/tmp/app.ts"); // Uint8Array
const text = new TextDecoder().decode(data);

await sb.checkpoint("my-env"); // saves disk state and stops the VM
```

### Start from a checkpoint

```ts
const sb = await Sandbox.start({ from: "my-env" });
```

### Options

```ts
const sb = await Sandbox.start({
  from: "my-env",
  cpus: 4,
  memory: 4096,
  diskSize: 8192,
  allowNet: true,
  ports: ["8080:80"],
  mounts: { "./src": "/workspace" },
  secrets: {
    API_KEY: { from: "OPENAI_API_KEY", hosts: ["api.openai.com"] },
  },
  network: { allow: ["api.openai.com", "registry.npmjs.org"] },
});
```

| Option | Type | Description |
|--------|------|-------------|
| `from` | `string` | Checkpoint name to start from |
| `cpus` | `number` | Number of vCPUs |
| `memory` | `number` | Memory in MB |
| `diskSize` | `number` | Disk size in MB |
| `allowNet` | `boolean` | Enable network access |
| `ports` | `string[]` | Port forwards (`"host:guest"`) |
| `mounts` | `Record<string, string>` | Directory mounts (`{ hostPath: guestPath }`) |
| `secrets` | `Record<string, SecretConfig>` | Secrets to inject via proxy (see below) |
| `network` | `NetworkConfig` | Network access policy (see below) |
| `shuruBin` | `string` | Path to shuru binary (default: `"shuru"`) |

## API

### `Sandbox.start(opts?): Promise<Sandbox>`

Boot a new microVM. Returns when the VM is ready.

### `sandbox.exec(command): Promise<ExecResult>`

Run a shell command in the VM. Returns `{ stdout, stderr, exitCode }`.

### `sandbox.readFile(path): Promise<Uint8Array>`

Read a file from the VM. Returns raw bytes. Use `new TextDecoder().decode(data)` for text files.

### `sandbox.writeFile(path, content: Uint8Array | string): Promise<void>`

Write a file to the VM. Accepts raw bytes or a string.

### `sandbox.checkpoint(name): Promise<void>`

Save the VM's disk state and stop the VM. To continue working, call `Sandbox.start({ from: name })`.

### `sandbox.stop(): Promise<void>`

Stop the VM without saving. All changes are discarded.

### Secrets

Secrets keep API keys on the host. The guest receives a random placeholder token; the proxy substitutes the real value only on HTTPS requests to the specified hosts.

```ts
const sb = await Sandbox.start({
  allowNet: true,
  secrets: {
    API_KEY: { from: "OPENAI_API_KEY", hosts: ["api.openai.com"] },
  },
});
// Inside the VM, $API_KEY is a placeholder token.
// Requests to api.openai.com get the real key injected by the proxy.
```

### Network policy

Restrict which domains the guest can reach:

```ts
const sb = await Sandbox.start({
  allowNet: true,
  network: { allow: ["api.openai.com", "*.npmjs.org"] },
});
```

Omit `network.allow` to allow all domains.

## Requirements

- macOS 14+ on Apple Silicon
- [shuru CLI](https://github.com/superhq-ai/shuru) installed
- Bun runtime
