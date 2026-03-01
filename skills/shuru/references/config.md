# Project Config (shuru.json)

Place `shuru.json` in the project root (or pass `--config <path>`). All fields are optional.

## Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cpus` | number | 2 | Number of CPU cores |
| `memory` | number | 2048 | Memory in MB |
| `disk_size` | number | 4096 | Disk size in MB |
| `allow_net` | boolean | false | Enable networking |
| `ports` | string[] | [] | Port forwards, `"HOST:GUEST"` format |
| `mounts` | string[] | [] | Directory mounts, `"HOST:GUEST"` format |
| `command` | string[] | ["/bin/sh"] | Default command to run |

## Resolution Order

CLI flags take priority over config values. Config values take priority over hardcoded defaults.

```
CLI flag > shuru.json > default
```

For example, `shuru run --cpus 4` with `{"cpus": 2}` in shuru.json uses 4 CPUs.

## Example

```json
{
  "cpus": 4,
  "memory": 4096,
  "disk_size": 8192,
  "allow_net": true,
  "ports": ["3000:3000", "8080:80"],
  "mounts": [".:/workspace"],
  "command": ["/bin/sh", "-c", "cd /workspace && sh"]
}
```

With this config, `shuru run` is equivalent to:

```bash
shuru run --cpus 4 --memory 4096 --disk-size 8192 --allow-net -p 3000:3000 -p 8080:80 --mount .:/workspace -- /bin/sh -c 'cd /workspace && sh'
```
