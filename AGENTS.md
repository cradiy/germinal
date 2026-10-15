# Testing discipline

These instructions apply to the entire repository.

- Write fewer, higher-value tests. Adding a test is not a mandatory step for every change.
- Before adding or expanding any test, identify the concrete bug, meaningful behavior, or important invariant it protects. Check whether existing coverage already protects it and whether the benefit justifies the setup and maintenance cost. If there is no clear value, do not write the test.
- Prefer updating an existing relevant test over creating another test for the same behavior.
- Do not test constants, default field values, getters, simple assignments, direct field forwarding, or obvious enum/boolean mappings in isolation.
- Do not copy the implementation into assertions or compute expected results using the same logic being tested. Do not add tests merely to increase coverage or test counts.
- Avoid checks that only assign or inject a value and assert that the same value was stored, unless a concrete integration failure makes that path worth testing.
- Retain focused coverage for real regressions, boundary conditions, state transitions, protocol handling, rendering correctness, and data safety. A short test is not automatically useless, and a long test is not automatically valuable.
- Keep manual previews and diagnostic fixtures separate from automated regression tests. Do not accumulate unrelated assertions in a preview or smoke test.
- When removing a redundant test, also remove its unused fixtures and setup. Do not replace it with another trivial assertion.
- Prefer headless verification. Avoid lengthy GUI runs that interrupt the user, and do not inject diagnostic input into an existing user terminal.
- Run relevant existing checks without inventing new tests for trivial changes. Documentation-only edits do not require Rust builds or test runs.
