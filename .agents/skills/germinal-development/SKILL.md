---
name: germinal-development
description: Develop, debug, test, lint, and review the Germinal Rust workspace. Use for changes to the terminal app, PTY behavior, wgpu rendering, workspace panes and tabs, configuration, Kitty compatibility, GNative crates, examples, or release builds in the Germinal repository.
---

# Germinal Development

Work from the repository root and preserve unrelated changes.

## Respect project boundaries

- Keep the dependency direction centered on `domain` -> `ports` -> `application` -> `infra` -> `app`.
- Put terminal parsing, PTY, platform, wgpu, image, video, and window-system implementations in `crates/infra`.
- Keep application orchestration in `crates/application`, interfaces and shared DTOs in `crates/ports`, and business state in `crates/domain`.
- Treat `gnative_protocol`, `gnative_core`, `gnative_sdk`, `gnative_ui`, `gnative_widgets`, and `gnative_demo` as a separate structured-UI stack.
- Do not change or version the GNative protocol unless the user explicitly requests it.
- Use stable Rust, the repository's default rustfmt output, and existing dependency versions unless a dependency change is required by the task.
- Put experimental rendering or public-API work in a separate example until its interface is intentionally stabilized.

## Develop a change

1. Inspect `git status --short` and the affected crate before editing.
2. Trace behavior across the relevant boundary instead of patching only the visible symptom.
3. Make the smallest coherent change and preserve unrelated working-tree edits.
4. Add or update focused tests for behavior that can be verified without a real window or compositor.
5. Run the narrowest relevant checks first, then the full workspace gates.
6. For rendering, input, compositor, PTY, image, or layout changes, run Germinal and verify the real behavior. Compilation alone is not visual or runtime acceptance.

## Run Germinal

Use the debug build during development:

```bash
cargo run --locked -p germinal
```

Use the optimized build when checking release behavior:

```bash
cargo run --locked --release -p germinal
```

The user configuration is at `~/.config/germinal/config.toml`. Avoid rewriting it during diagnostics; use an isolated `XDG_CONFIG_HOME` when a temporary configuration is needed.

Run repository examples explicitly:

```bash
cargo run --locked -p germinal --example kitty_image
cargo run --locked -p germinal --example two_pane
```

Start `germinal-gnative-demo` from a PTY inside a running Germinal instance so the injected tunnel endpoint, token, and protocol environment are present:

```bash
cargo run --locked --release -p germinal-gnative-demo
```

Do not invent or hard-code missing `GERMINAL_GNATIVE_*` environment values when the demo is launched from an ordinary terminal.

## Test narrowly, then broadly

Run a focused crate or test while iterating:

```bash
cargo check -p germinal-infra
cargo test -p germinal-infra path::to::test_name
```

Before handoff or commit, run all repository gates:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

If a command cannot run because the host lacks a display, compositor, GPU, multimedia library, or another system dependency, report the exact limitation. Do not claim that runtime or visual behavior was verified.

## Enforce zero-warning Clippy

- Treat every Clippy warning as an error by always passing `-- -D warnings`.
- Fix the underlying code; do not add broad `#[allow(clippy::...)]` attributes merely to make the command pass.
- Use a narrowly scoped allow only when the lint is intentionally inapplicable and record the reason next to it.
- Run Clippy with `--workspace --all-targets --all-features` so libraries, binaries, tests, and examples obey the same rule.
- Do not hand off or commit with a known Clippy warning.

## Commit safely

- Review `git status --short`, `git diff --check`, and the staged diff.
- Stage explicit paths so unrelated work is not included.
- Report the exact checks and runtime verification performed.
