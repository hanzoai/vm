# RFC 0002: Refreshable secrets

- Status: Draft
- Date: 2026-08-04
- Scope: shuru-proxy, shuru-cli config, shuru-sdk public types
- Issue: superhq-ai/shuru#40

## Summary

Let a secret be backed by a command instead of a host environment variable.
The proxy runs that command to mint the real value, caches it in memory, and
re-runs it when the value expires. The placeholder in the guest never changes,
so a rotation is invisible inside the VM.

This makes short lived credentials usable for long running sandbox work.
The motivating case is a GitHub App installation token, which dies after one
hour and currently strands any agent task that outlives it.

## Motivation

Secrets resolve from the host environment of the shuru process, which is
frozen at exec. A VM that runs for ninety minutes against a sixty minute
token fails with an auth error after most of the work is already done. The
only workaround today is to split the task across VM boots using checkpoints.

The alternative users reach for otherwise is a long lived personal access
token, which is strictly worse: broader scope, no expiry, and a much larger
blast radius if the host is compromised.

Refreshing on the host side preserves the property the proxy exists to
provide. Real credentials stay outside the VM, the guest holds a placeholder
it cannot redeem anywhere except the bound hosts, and now the guest cannot
retain anything useful even in the window where it holds a live session.

## Non-goals

- A control socket or `shuru secret set` command. That needs a per VM control
  plane, which does not exist and should not be motivated by secrets alone.
- File watching. Secrets at rest plus partial write races, for no benefit
  over a command.
- A CLI flag for command backed secrets. The `--secret NAME=ENV@hosts` syntax
  does not extend to argv without quoting hazards. Config file only.
- Refreshing on an upstream 401. Expiry driven refresh covers the reported
  case; reactive refresh is future work.
- Re-resolving inside a live connection. See the granularity note below.

## Background

Secrets flow through three points today.

`ProxyConfig.secrets` maps a guest visible env var name to a `SecretConfig`
with `from` (host env var), `hosts` (domains where substitution is allowed),
and an optional literal `value` (`crates/shuru-proxy/src/config.rs:39`).

At proxy start, one random placeholder per secret is generated
(`crates/shuru-proxy/src/lib.rs:109`) and handed to the guest as the value of
the env var at exec time (`crates/shuru-cli/src/vm.rs:418`,
`crates/shuru-sdk/src/lib.rs:963`).

At TLS connection setup, `secrets_for_domain` matches the SNI against the
bound hosts and reads `std::env::var(&secret.from)` to build a list of
placeholder to real value pairs (`crates/shuru-proxy/src/config.rs:104`).
That list is captured once and passed into `handle_mitm`, which byte replaces
the placeholder in every chunk flowing guest to upstream
(`crates/shuru-proxy/src/proxy.rs:352`).

So the value is already re-read per connection rather than cached at launch.
It simply never changes, because the process environment cannot change.
Mutating it is not an option either: `std::env::set_var` is unsafe in a
multithreaded process under the 2024 edition, and the proxy runs on its own
threads.

Prior art converges on the same answer. The AWS `credential_process` runs a
command that prints JSON with an `Expiration` field, and the SDK re-runs it
when the clock passes that time. Git credential helpers added
`password_expiry_utc` in 2.41 for exactly the GitHub App case. Docker
credential helpers pass the target host in on stdin but have no expiry
concept, so they pay a subprocess spawn on every operation. This design takes
the AWS shape, since the bound hosts are already known from config and there
is nothing to pass in.

## Design

### Config surface

`SecretEntry` gains `command` and `ttl`, and `from` becomes optional:

```json
{
  "allow_net": true,
  "secrets": {
    "GITHUB_TOKEN": {
      "command": ["./scripts/mint-installation-token.sh"],
      "hosts": ["api.github.com", "github.com"]
    },
    "API_KEY": {
      "from": "OPENAI_API_KEY",
      "hosts": ["api.openai.com"]
    }
  }
}
```

Exactly one of `from` or `command` must be set. Both set, or neither, is a
config error reported at load time rather than at first request.

`command` is an argv array, not a shell string. No shell sits between shuru
and a credential, and there is no quoting hazard for paths with spaces.

`ttl` is an optional duration string (`"45m"`, `"3600s"`) used only when the
command reports no expiry of its own.

### Provider contract

The command runs with stdin closed, inheriting the host environment of the
shuru process, with the working directory set to the directory containing the
resolved config file so relative program paths and relative paths inside the
script both behave predictably.

On success it exits zero having written one JSON object to stdout:

```json
{
  "version": 1,
  "value": "ghs_...",
  "expires_at": "2026-08-04T18:36:00Z"
}
```

- `version` must be 1. Unknown versions are a resolve failure, which reserves
  room to change the shape later.
- `value` is the credential, used verbatim. Trailing newline is trimmed.
- `expires_at` is optional RFC3339. Present means refresh at that time.
  Absent falls back to `ttl`. Absent with no `ttl` means fetch once and never
  refresh, matching `credential_process`.

Any of a non-zero exit, unparseable stdout, an empty `value`, or a timeout is
a resolve failure. The default timeout is 10 seconds and the process is
killed on expiry.

Stderr is captured, truncated to 4 KB, and surfaced only in the resolve
failure message. Stdout is never logged at any level, including under
`--verbose`, since it holds the credential.

### Resolution and caching

A new `SecretResolver` in shuru-proxy owns what `ProxyConfig` cannot, since
the engine holds the config as an immutable `Arc<ProxyConfig>`
(`crates/shuru-proxy/src/proxy.rs:27`):

```rust
pub struct SecretResolver {
    config: Arc<ProxyConfig>,
    placeholders: HashMap<String, String>,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

struct CacheEntry {
    value: String,
    expires_at: Option<Instant>,
}
```

`secrets_for_domain` becomes an async method on the resolver with the same
return type, called from the same place at connection setup
(`crates/shuru-proxy/src/proxy.rs:229`). `handle_mitm` keeps its existing
`Vec<(String, String)>` parameter and needs no change.

Env backed secrets keep reading `std::env::var` with no caching, which is
free and preserves current behavior exactly.

Command backed secrets resolve against the cache:

1. A cached entry with no expiry, or an expiry more than the refresh window
   away, is returned as is.
2. Otherwise run the command, store the result, return it.

The refresh window is 60 seconds, so a value is re-minted shortly before it
dies rather than at the first request that fails.

Concurrent connections to the same host must not each spawn a process. The
cache mutex is held across the fetch for that secret, so the first caller
runs the command and the rest wait and observe the fresh entry. The mutex is
per resolver rather than per secret, which serializes two different expiring
secrets in the rare case they lapse together. That is acceptable against a 10
second worst case and avoids a map of in-flight futures.

### Refresh granularity

Substitution values are captured once per TLS connection. A connection held
open across an expiry boundary keeps using the value it started with.

This is sufficient for the motivating case. An agent doing `git push` or an
API call ninety minutes in opens a new connection, which resolves fresh. The
gap is a single long lived keep-alive or HTTP/2 connection that spans the
rotation, where the guest sees one auth failure and gets a working value on
reconnect.

Closing that gap means re-reading the cache inside the relay loop, which
needs a lock free read on the hot path (an `ArcSwap` load per chunk) plus a
background refresh task, since the loop cannot await a subprocess. Deferred
until something demonstrates it matters.

### Failure handling

| Condition | Behavior |
| --- | --- |
| Both or neither of `from` and `command` set | Config error at load |
| Command fails, no cached value | Fail the connection with a clear error |
| Command fails, unexpired value cached | Serve the cached value, log under verbose |
| Command fails, cached value already expired | Fail the connection |
| Env var missing (`from` secrets) | Skip substitution, unchanged behavior |

Command backed secrets fail closed. Forwarding a placeholder to GitHub
produces a confusing 401 that reads like a scope problem, and there is no
reason to spend an upstream request on a value known to be wrong.

Env backed secrets keep the existing skip behavior so nothing that works
today starts failing. The inconsistency is deliberate and noted below.

## Security

The credential still never enters the VM. The placeholder is generated once
per proxy and injected at exec, so a refresh changes only the host side
substitution table and the guest observes nothing.

Refreshing narrows the window. A guest that exfiltrates a live session gains
nothing durable, because the value it was transiting expires on schedule and
the replacement is minted outside the VM. Compared against the long lived PAT
this replaces, that is a clear improvement.

New surface is one subprocess spawned by the host, from an argv array in a
config file the user controls, with no guest input reaching it. A hostile
`shuru.json` can already set `command` in the top level config and run
arbitrary code on `shuru run`, so this adds no capability that did not exist.

Handling rules: the value never appears in logs, in error messages, or in
`ps` output, and the cache is memory only with nothing written to disk.

## Testing

Unit:

- Expiry math, including the refresh window and the `expires_at` over `ttl`
  over never precedence
- Provider output parsing: version mismatch, missing `value`, trailing
  newline, garbage stdout
- Config validation for both and neither of `from` and `command`
- Cached value served without re-running the command

Integration, with a script that writes a different value on each call:

- Two connections separated by an expiry get different upstream values
- Concurrent connections to one expired secret spawn the command once
- A failing command fails the connection rather than forwarding a placeholder
- A failing command with an unexpired cached value keeps serving it
- The guest env placeholder is byte identical before and after a refresh

## Rollout

Additive. Existing configs use `from` and behave exactly as they do today.

`SecretConfig` is re-exported from shuru-sdk
(`crates/shuru-sdk/src/lib.rs:12`), so the field changes are a breaking change
for SDK consumers building it as a struct literal. It gains `Default`, the
two literals in shuru-cli move to struct update syntax
(`crates/shuru-cli/src/vm.rs:100`, `crates/shuru-cli/src/config.rs:46`), and
shuru-proxy plus shuru-sdk take a minor bump.

Documentation lands in `skills/shuru/references/config.md` alongside the
existing secrets section, with the GitHub App token as the worked example.

## Open questions

- Whether env backed secrets should also fail closed on a missing value. It
  is the more defensible behavior and the current skip is closer to an
  oversight than a decision, but changing it breaks configs that tolerate an
  unset variable today.
- Whether `ttl` earns its place given that any provider worth writing can
  report `expires_at`. It exists for wrapping a command that cannot, such as
  a bare `gh auth token`.

## Future work

- Reactive refresh on an upstream 401, which covers providers that revoke
  early and removes the dependence on accurate expiry reporting.
- Mid-connection refresh via `ArcSwap` plus a background refresh task.
- A `shuru secret set` control command, once a general per VM control plane
  exists for port exposure and network policy changes.

## Note on an unrelated defect

`replace_bytes` runs per read chunk (`crates/shuru-proxy/src/proxy.rs:352`),
so a placeholder split across two TCP segments is forwarded unsubstituted.
This predates this RFC and is not made worse by it, but it lives in the code
this change touches and deserves its own fix: carry a tail buffer of
placeholder length minus one byte across chunk boundaries.
