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
| `crates/voice` | `aginx-voice` | voice dialog daemon — PTT input, closed-vocab protocol, face writer |
| `crates/wizard` | `aginx-net-wizard` | first-boot Wi-Fi setup TUI |
| `crates/term` | `aginx-term` | on-device terminal UI (launcher + pty shell on the panel) |
| `crates/pkg` | `aginx-pkg` | package manager — signed manifest, 四件套 tars |
| `crates/svc` | `aginx-svcd`/`aginx-svc`/`aginx-boot-ok` | supervisor, control client, A/B slot marker |
| `crates/sign` | `aginx-sign` | host-side ed25519 signer/verifier |
| `crates/qr` | `aginx-qr` | QR decode CLI — quircs + jpeg decode face (built in its own zigbuild pass) |
| `crates/img` | `aginx-img` | vendored libjpeg-turbo decode (shared FFI) |
| `crates/download` | `aginx-download` | HTTPS downloader — streaming, .part+rename |
| `crates/update` | `aginx-update` | signed A/B rootfs updater — swap + state tar |
| `crates/done` | `aginx-done` | provision done markers |
| `crates/secret` | `aginx-secretd`/`aginx-secret` | secret sidecar daemon + admin face |
| `crates/gateway` | `aginx-gateway` | remote channel — registers to relay.aginx.net, collapses external JSON-RPC onto the server's UDS front |

Build entry points:

- `./scripts/check.sh` — host gate (tests + registry lint), before every commit
- `./scripts/build-rootfs.sh` — bake the flashable image (`out/rootfs.img`;
  first-gen assets referenced via `OLD=`), see `rootfs/README.md`
- `./scripts/accept/n4.sh` — device acceptance (the N4 switch gate)
- `./scripts/accept/n5.sh` — device acceptance (N5: absorption, state
  migration, backup line, remote-channel receipt via host `agc`)

Milestones are the N series (see AGENTS.md). First-generation repo:
`~/Documents/aginxos` — asset library since N4 (busybox, C tool sources,
vendor ramdisk, voice/OCR builds, blobs, pre-N4 receipts).
