# GNative over SSH

## Status

This document defines the implementation plan for running a GNative application on a remote
machine while presenting it as the active view of a local Germinal pane. It is a design only; the
current implementation remains local-only.

The first target is Linux-to-Linux over OpenSSH. WSL, Mosh, roaming sessions, and cross-platform
remote hosts are deferred until the SSH transport is stable.

## Intended workflow

```text
local Germinal
  $ germinal ssh user@example.com

remote shell
  $ cargo run --release -p my-gpui-app
```

The remote shell behaves like an ordinary SSH shell until a GNative-aware application starts. The
application replaces the terminal view in the same pane, receives pane input, and returns the pane
to PTY mode when it exits or disconnects.

Remote applications continue to call `gpui_germinal::application()`. Application code does not
select shared memory, compression, or SSH explicitly.

## Goals

- Present a remotely executed headless GPUI/GNative application in a local Germinal pane.
- Preserve keyboard, pointer, scrolling, IME, focus, paste, and resize behavior.
- Keep shared memory as the fast local default.
- Transmit no surface bytes while the view is unchanged.
- Bound memory and latency when the producer is faster than the connection.
- Reuse the system OpenSSH client, configuration, agent, and host-key policy.
- Return to a usable PTY after every connection or protocol failure.

## First-version non-goals

- Transparent reconnection or session migration.
- Mosh support.
- Remote shared GPU textures.
- WSL-specific launch, GPU, or networking support.

## Current local path

```text
GPUI scene
  -> headless WGPU render
  -> RGBA readback
  -> three-slot mmap file in XDG_RUNTIME_DIR
  -> SharedRgbaSurface metadata over GNative TCP
  -> validated mmap in Germinal
  -> local WGPU texture upload
```

The GNative control channel is already a token-authenticated TCP listener bound to `127.0.0.1`.
Only `SharedRgbaSurface` requires Germinal and the application to see the same file. A remote mmap
path is meaningless on the local host, so forwarding the existing TCP port alone cannot render a
remote surface.

## Target architecture

```text
Remote host                                         Local host

GPUI application
  -> headless WGPU
  -> RGBA readback
  -> changed-tile detection
  -> zstd tile encoder
  -> GNative network surface ---- SSH reverse ----> GNative tunnel
                                  forwarding          -> tile decoder
                                                      -> persistent texture
                                                      -> Germinal pane

GPUI input handler <------------ same stream <----- keyboard/pointer/IME/resize
```

One application-facing surface interface has two negotiated implementations:

| Transport | Use | Data path |
| --- | --- | --- |
| `SharedRgbaV1` | Application and Germinal are local | Three-slot mmap |
| `ZstdTilesV1` | Application is reached through SSH | Compressed changed tiles |

The local path must not pay hashing or compression costs.

## SSH connector

Add a `germinal ssh` command, or a packaged `germinal-ssh` executable if a GUI subcommand would
complicate the main entry point. It reads the pane's `GERMINAL_GNATIVE_*` environment and launches
the user's system `ssh` executable.

The connector owns this reverse forwarding rule:

```text
remote 127.0.0.1:<allocated-port> -> local 127.0.0.1:<gnative-listener-port>
```

Requirements:

- Bind both endpoints to loopback.
- Use `ExitOnForwardFailure=yes`.
- Let OpenSSH allocate a remote port and obtain it through a controlled master session; do not
  reserve a fixed global port.
- Bootstrap the remote login shell with the forwarded endpoint, token, protocol version, and
  `GERMINAL_GNATIVE_SURFACE_TRANSPORT=network`.
- Preserve ordinary OpenSSH arguments and configuration, including aliases, `ProxyJump`, identity
  files, SSH agents, and host-key checks.
- Never add `StrictHostKeyChecking=no` or expose a forwarding endpoint on a public interface.
- Do not require `SendEnv`/`AcceptEnv`; the connector installs the remote environment itself.
- Avoid redundant SSH compression for the already zstd-compressed surface channel.

No permanent remote daemon is required initially. A small remote bootstrap helper remains an
option only if allocated-port discovery or environment setup proves unreliable across supported
OpenSSH versions.

## Protocol negotiation

The current protocol remains unchanged while the new transport is developed behind tests. The
GNative protocol version changes only after SDK and host support are complete.

The upgraded handshake conceptually includes:

```rust
struct GNativeAppHello {
    token: String,
    protocol_version: u32,
    surface_transports: Vec<SurfaceTransportCapability>,
}

struct GNativeSessionAccepted {
    gshell_id: GShellId,
    protocol_version: u32,
    surface_transport: SurfaceTransportKind,
}
```

Selection rules:

1. A local application advertises both transports; Germinal prefers `SharedRgbaV1`.
2. SSH bootstrap makes the remote client advertise `ZstdTilesV1` only.
3. No common transport rejects GNative entry with an actionable error.
4. A network session can never submit a host file path as a surface payload.
5. The selected transport is immutable for one session.

## Binary wire framing

Newline-delimited JSON remains unsuitable for pixel bytes because `Vec<u8>`
becomes a large integer array. The upgraded channel uses bounded length-prefixed binary messages:

```text
+------------+-----------+-----------+---------------------+
| body bytes | kind      | flags     | MessagePack body    |
| u32 BE     | u8        | u8        | binary byte payloads|
+------------+-----------+-----------+---------------------+
```

- Byte fields are binary blobs, never Base64 or JSON arrays.
- The existing stream kind, priority, and monotonic mux sequence remain.
- Encoded bytes, decompressed bytes, dimensions, tile counts, and queue bytes have separate limits.
- Decoding rejects invalid lengths, trailing data, integer overflow, and invalid enum values before
  creating render resources.

## Network surface protocol

### Lifetime

The host keeps a persistent texture. The client sends:

1. `SurfaceCreate`: ID, dimensions, `RGBA8 sRGB`, tile size, and epoch.
2. A full keyframe.
3. `SurfaceTiles`: changed tiles for a frame generation.
4. `SurfaceReset`: new epoch after resize or recovery.
5. `SurfaceRelease`: application or session teardown.

Germinal ignores old epochs and generations. Alpha semantics must match the local surface and have
an explicit transparent-pixel fixture before the format is finalized.

### Encoding

- Start with 64 by 64 pixel tiles; edge tiles may be smaller.
- Hash each tile after RGBA readback and compare it with the previous submitted frame.
- Compress only changed tiles with zstd level 1.
- Batch descriptors and compressed byte ranges up to a bounded wire-message size.
- Send a full keyframe on create, resize, invalidation, or host recovery request.
- Keep tile size and zstd level fixed until profiling proves negotiation is useful.

### Flow control

- Permit at most two unacknowledged surface generations.
- If the limit is reached, retain only the newest complete RGBA frame and discard intermediate
  frames before compression.
- Germinal sends `SurfaceAck` after validation, decompression, and scheduling the texture update.
- A missing acknowledgement requests a keyframe or closes the session; it never expands a queue.
- Control and input must not wait behind a large surface payload.

This makes the connection latest-frame-wins: a slow link lowers frame rate instead of adding
unbounded input latency.

## Code ownership

| Responsibility | Owner |
| --- | --- |
| Capability DTOs, binary envelopes, limits, surface messages | `crates/gnative_protocol` |
| Client framing, handshake, queues, acknowledgements | `crates/gnative_sdk` |
| Host validation, decompression, persistent surface resources | `crates/infra` |
| Tunnel interfaces | `crates/ports` |
| Session and failure orchestration | `crates/application` |
| OpenSSH process and pane lifecycle | `app` or connector binary |
| GPUI transport selection, hashing, compression | `gpui-germinal` |

`gpui`, `gpui_linux`, and `gpui_wgpu` remain unaware of SSH. Their boundary is headless rendering
and RGBA readback.

## Failure behavior

| Failure | Required result |
| --- | --- |
| Reverse forwarding fails | Stay in PTY mode and print the OpenSSH error |
| No common transport | Reject entry and list supported transports |
| Remote application exits | Release surfaces and return to PTY mode |
| SSH disconnects | Drop queues, release surfaces, return to PTY mode |
| Malformed/oversized message | Close only that GNative session and report why |
| Tile generation gap | Keep last valid texture and request a keyframe |
| Resize | Start a new epoch and require a keyframe |
| No remote WGPU adapter | Report startup failure while PTY stays usable |

Other panes, terminal processes, and local GNative sessions must survive every remote failure.

## Security requirements

- Replace the timestamp token with at least 256 bits from the OS cryptographic random source before
  exposing a forwarded endpoint.
- Bind local and remote endpoints to loopback.
- Use OpenSSH for authentication and encryption; do not add a second SSH implementation.
- Treat every remote protocol message as untrusted after authentication.
- Validate dimensions, strides, rectangles, offsets, compressed and decompressed sizes, epochs,
  and generations before allocation or copy.
- Bound decompression output to prevent compression bombs.
- Never open a path supplied by a network-negotiated session.
- Pass SSH options as argument-vector values. Do not concatenate unescaped user input into a remote
  shell command.
- Remove forwarding and temporary control sockets when the pane closes.

## Measurements and acceptance

Record separately for mmap and SSH:

- WGPU render and readback time.
- Hashing and compression time.
- Bytes per frame and second.
- Decode and texture-upload time.
- Produced, sent, presented, coalesced, and dropped frame counts.
- Input-to-present latency at p50 and p95.
- Outstanding generations and queued bytes.

Test 1280x720 and 1920x1080 with a static screen, text editing, list scrolling, a small animation,
and a full-window animation stress case.

Initial acceptance requirements:

- A settled static screen sends no recurring surface payload.
- Queued memory remains bounded under bandwidth throttling.
- Input stays responsive while intermediate frames are dropped.
- A keyframe reconstructs an exact pixel copy of a deterministic fixture.
- Local mmap performance does not regress by more than five percent in repeated measurements.
- Disconnect and malformed-message tests return the pane to PTY without crashing Germinal.

LAN and WAN frame-rate targets are set after the first instrumented prototype, not guessed in
advance.

## Implementation phases

### Phase 0: Baseline

- Add deterministic RGBA fixtures for transparency, gradients, sharp text-like edges, and resize.
- Instrument current local production, presentation, drops, timing, and bytes.
- Preserve a repeatable benchmark/demo command.

Exit: pixel fixtures pass and local baseline numbers are recorded.

### Phase 1: Surface abstraction

- Extract mmap behind a GPUI-side surface transport interface.
- Split host surface lifetime from individual frame updates.
- Select `SharedRgbaV1` only.
- Test selection, reset, release, and stale generations.

Exit: local behavior is equivalent and performance stays within baseline tolerance.

### Phase 2: Binary protocol and capability negotiation

- Add bounded binary framing and binary byte fields.
- Add capability advertisement and host selection.
- Preserve mux ordering and priorities.
- Test round trips, partial reads, bad lengths, and explicit version rejection.
- Increment the protocol version only when host and SDK are both ready.

Exit: the local demo completes a session through the new framing.

### Phase 3: Compressed tile surface

- Add persistent textures, tile hashing, zstd, partial texture updates, acknowledgements, frame
  coalescing, recovery keyframes, and resize epochs.
- Exercise it over ordinary loopback TCP with artificial latency and bandwidth limits before SSH.

Exit: loopback network mode passes pixel, flow-control, resize, and failure tests.

### Phase 4: OpenSSH connector

- Add connector and reverse-forward lifecycle.
- Bootstrap the remote environment without server `AcceptEnv` changes.
- Preserve user SSH configuration and options.
- Diagnose forwarding policy, host-key, authentication, and bootstrap failures.
- Test localhost OpenSSH and at least one separate Linux host.

Exit: a remote GPUI demo enters GNative mode, handles all input classes and resize, exits, and
returns to the same PTY pane.

### Phase 5: Optimization

- Tune tile size, batches, and worker allocation from profiles.
- Investigate GPUI damage propagation to avoid hashing a complete readback.
- Document measured LAN and WAN operating ranges.

Exit: common UI interaction meets measured targets without regressing local mode.

## Validation matrix

Every implementation phase runs:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

SSH runtime validation additionally covers key/password/agent authentication, direct and
`ProxyJump` hosts, disabled remote forwarding, normal exit, crash, connection loss, pane closure,
all input classes, resize, static and changing surfaces, and isolation from a second local pane.

## Deferred decisions

- Whether to ship a remote `germinal-agent`.
- Whether 64 by 64 remains the best tile size.
- Whether MessagePack eventually needs a schema-generated replacement.
- Whether GPU-native encoding is worthwhile for full-window animation.
- Whether reconnection should preserve the last surface.
- WSL launch, socket, GPU, and localhost-forwarding behavior.
