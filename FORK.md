# Personal Herdr build

This fork keeps Brandon's sidebar navigation on upstream Herdr **v0.9.1**.
Upstream releases come from https://github.com/herdrdev/herdr.

The client-owned tree keeps colored machine names, numbered pane shortcuts,
name-only labels, hidden redundant tab headings, and a single space/pane row
when a space contains only one tab and one pane. `ui.hide_tab_bar` hides the
separate desktop tab strip. Pane-tree navigation and compact/mobile workspace
navigation remain separate; remote worktree collapses are scoped per machine.

## Build and install

Herdr 0.9.1 requires Zig **0.16.0**, Rust, and `just`. Use a separate worktree
under the checkout's `.worktrees` for upgrades and a separate Cargo target
directory per source worktree. Merge the upstream release tag, preserve the
client navigation, and run the checks before installing.

On work-mac the build environment is:

```sh
export PATH="$HOME/.local/lib/herdr-build-tools/sdk15-bin:/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
export ZIG="$HOME/.local/lib/herdr-build-tools/zig-aarch64-macos-0.16.0/zig"
just check
HERDR_BUILD_CHANNEL=custom HERDR_BUILD_ID="brs98-$(git rev-parse --short HEAD)" just build
```

Do not set custom build identity variables while running tests that check the
stable version string. Windows cross-lint additionally needs the SDK described
in AGENTS.md; native builds do not require it.

The installed binary is `~/.local/lib/herdr-fork/herdr`, behind the launcher
`~/.local/bin/herdr`. Install to a temporary sibling file and rename it atomically.
The launcher rejects `herdr update` so upstream updates or the older pinned Linux
installer cannot replace the fork. Homebrew and system packages remain separate.
A dotfiles restow can restore an older launcher; check the fork pin before using
its updater again.

Updating the binary does not update an existing server or attached client.
Live handoff is experimental: verify the old/new binary pair with a disposable
session, preserve backups, and compare pane IDs and shell PIDs after the handoff.
Never stop a live server merely to install a client update. Reopen attached
clients to load the new client code. The upstream macOS logout/DNS fix requires
a fresh server session; live handoff cannot repair an inherited service context.

## Navigation

```toml
[keys]
jump_sidebar_item = ["alt+1..9", "alt+0"]
jump_sidebar_item_prompt = "alt+j"

[ui.sidebar.spaces]
rows = [["state_icon", "workspace"]]

[ui.sidebar.agents]
rows = [["state_icon", "pane"]]
```

Open navigation with prefix then `w`. Use `j`/`k` to move, `h`/`l` to
collapse/expand, `/` to search, and Enter to focus. `Alt+0` selects pane 10;
`Alt+j` accepts a multi-digit pane number.

## Upgrade validation (2026-09-25)

- Merged upstream v0.9.1 into the fork; retained the custom navigation and labels.
- Native lint and all 3,483 runnable Rust tests passed. The seven skipped manual
  rendering profiles passed separately with `just bench-render-scale`.
- Maintenance, UI architecture, integration assets, and documentation contracts
  passed. `just check` reached Windows lint but could not run that stage because
  this Mac has no configured Windows SDK; this deployment targets macOS ARM64
  and Linux x86_64 only.
- Sidebar rendering at fixed geometry averaged 112.38 microseconds with one
  pane and 125.80 with 15 panes (about 12% growth).
- Disposable 0.9.0-to-0.9.1 handoffs passed on macOS and Linux, preserving shell
  PIDs, scrollback, and terminal input.
- Installed build identity: `0.9.1-custom.brs98-0.9.1-navigation`.
- Previous binaries, launchers, saved state, and pane inventories are backed up
  on each machine under `~/.local/lib/herdr-fork/backups/before-0.9.1-*`.

## Upgrade validation (2026-09-09)

- Rust suite: 3,122 passed, two upstream live-handoff tests failed on this Mac,
  and three manual benchmarks were excluded from the normal run.
- All 216 client tests passed, including tree, search, jump, machine identity,
  selection safeguards, token styling, scrollbar, drag, and notification checks.
- Formatting, Clippy, Windows target lint, 101 maintenance tests, and the
  architecture, integration asset, plugin marketplace, and documentation
  contract checks passed.
- All three release-mode rendering benchmarks passed. The custom client
  composition profile measured 111.44 microseconds with one populated agent
  pane and 126.78 microseconds with 15 (about 14% more) at fixed 120×40 geometry.

The two failures reproduce in unchanged upstream test/platform code:
`live_handoff_preserves_pane_process_io` sets a socket timeout after peer close,
which returns EINVAL on this macOS; `live_handoff_keeps_unmanaged_agent_name_bound_to_saved_session`
uses Apple's `/bin/sleep` as a fake agent, whose process arguments omit its
`HERDR_AGENT` environment hint here. Standalone socket/process probes reproduced
both OS behaviors. These tests were not disabled or changed to hide the failures.
