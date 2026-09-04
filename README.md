# AginxOS

OS for agents, second generation. A Linux machine where the platform is
one server (the mother, `aginx-server`), avatars are folders under
`~/.aginx/workspaces/` run by a single runtime engine (`aginx-runtime`)
over the fast-agi stdio protocol, and every external piece enters as a
CLI (`aginx-*`, file-is-registry, dispatched by the bare `aginx`
router). The constitution lives in `docs/ARCH.md` (local only).

| Crate | Binary | Role |
|-------|--------|------|
| `crates/router` | `aginx` | the bare command — mother's face, file-is-registry dispatch |
| `crates/server` | `aginx-server` | front desk (进/住/切/退), session cursor, request routing, session ledger |
| `crates/runtime` | `aginx-runtime` | fast-agi engine: runs an avatar folder |
| `crates/agi` | — | fast-agi v0 frame types |
| `crates/agio` | — | D1 output envelope for every CLI |

Host gate: `./scripts/check.sh`. Milestones are the N series (see
AGENTS.md). First-generation repo: `~/Documents/aginxos` (asset library
+ running device line until N4).
