# AginxOS — Agent Guide

Second-generation AginxOS: the architecture constitution (D1–D13) built
as a fresh workspace, host first. The previous repositories are asset
libraries, not the product:

- `~/Documents/aginxos` — first-generation device line. It owns the
  running Pixel 5 (redfin) until this repo can bake a flashable image
  (N4). Device facts live only in its `docs/HARDWARE.md`; never invent
  hardware state. Do not change its code — it runs the phone today.
- `~/Documents/aginx` — ecosystem (aginx-carrier, aginx daemon,
  aginxbrowser, memory server). Source of import seams (agent_loop,
  llm_driver, bridges), not of ongoing development.

## Constitution

`docs/ARCH.md` is the architecture constitution: one server per machine
(the mother, aginx), avatars are folders run by a single runtime engine,
display is request semantics, the session log is the truth source,
addressing is front-desk registration (进/住/切/退), externals are
CLI-only (D12), and every command carries the aginx surname (D13).
**ARCH.md is LOCAL ONLY — never commit or push it** (same treatment as
the old repo's ARCH/CARRIER/SYSTEM docs; `.gitignore` enforces it).

## Milestones (N series)

- **N1 platform heart, host loop**: `aginx` router + `aginx-server`
  (front desk, cursor, request routing, session ledger) +
  `aginx-runtime` (fast-agi engine) — closed loop with the real brain on
  the host.
- **N2 trial on device**: adb push, isolated HOME `~/.aginx-n`, the old
  carrier data untouched.
- **N3 coexisting package**: agpkg on the live image, daily-driver
  verification.
- **N4 bake takeover**: this repo inherits the boot/bake/flash chain and
  the old repo archives.

## Ground Rules

- Host green before anything: `./scripts/check.sh` (cargo test over the
  workspace + `aginx commands --check` registry lint) must pass before
  every commit.
- The avatar root is `~/.aginx-n/workspaces` during host/N2 trials
  (`AGINX_HOME` overrides); production shape is `~/.aginx/workspaces`.
  Trial runs must never touch the first-generation `~/.aginx` tree.
- Naming law D13: `aginx` is the only bare command (the router); every
  external command is `aginx-<domain>-<object>-<verb>`, verb last.
  Resident engine names (aginxbrowser, aginxbrain, aginxmemory) are
  service names and stay out of the command universe.
- Secrets never enter the repo or docs. Brain access is
  `AGINXBRAIN_API_KEY` in the environment at runtime — never committed,
  never echoed.
- Import, don't entangle: code brought in from the asset libraries gets
  re-housed on fast-agi frames and D13 names in the same commit it
  arrives; no compatibility shims to the old kernel types.

## Hosts & Toolchains

Host-first: everything in N1 builds and tests on macOS and Linux with
stable Rust. The musl device target (`aarch64-unknown-linux-musl`, zig /
cargo-zigbuild, fully static) arrives with N2 — do not add
device-only dependencies before then.

## Layout

| Path | What |
|------|------|
| `crates/router` | `aginx` — the bare command, mother's face |
| `crates/server` | `aginx-server` — front desk, cursor, routing, ledger |
| `crates/runtime` | `aginx-runtime` — fast-agi engine (avatar runner) |
| `crates/agi` | fast-agi v0 frame types (both ends share) |
| `crates/agio` | D1 output envelope |
| `shims/` | repo-local `aginx-*` command faces (host trial registry) |
| `docs/ARCH.md` | the constitution (local only, gitignored) |

## Git

- Atomic commits: one coherent change, imperative message.
- Every commit passes `./scripts/check.sh`.
- Device-behavior changes (N2+) get a receipt in the old repo's
  `docs/HARDWARE.md` until N4 moves the log here.
