# Personal Herdr build

This fork keeps Brandon's sidebar navigation on top of upstream Herdr v0.9.0.
Upstream releases come from https://github.com/herdrdev/herdr.

The navigation customization belongs in `src/client/shell/sidebar_tree.rs` and
related client UI code. Herdr 0.9 moved presentation out of the server; changes
to the old server-side sidebar will not affect the active client.

## Build and install on this Mac

```sh
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/opt/zig@0.15/bin:/opt/homebrew/bin:$PATH"
just --set python "$HOME/.local/bin/python3.11" check
just build
install -m 755 target/release/herdr "$HOME/.local/bin/herdr"
```

`~/.local/bin` precedes Homebrew in this Mac's PATH. Homebrew's official binary
remains available at `/opt/homebrew/bin/herdr`. Homebrew upgrades do not update
this personal build. To use a newer upstream release with the custom sidebar,
merge its tag into the fork and rebuild. Avoid `herdr update` on the personal
binary: an upstream binary does not contain the custom navigation.

On this Mac, the packaged Python 3.14 XML extension cannot load its Expat symbols;
the check command above uses the existing working Python 3.11 installation.

## Future upgrades

From a clean checkout, fetch and create a separate worktree for the next release:

```sh
git fetch upstream --tags
git worktree add -b upgrade/<version>-navigation ../herdr-worktrees/<version>-navigation main
```

In that worktree, merge the chosen release tag, resolve conflicts while keeping
the client navigation, run `just check`, and test the sidebar before installing.
Keep both the upstream release history and the fork changes in the merge.

Existing custom bindings:

```toml
[keys]
jump_sidebar_item = ["alt+1..9", "alt+0"]
jump_sidebar_item_prompt = "alt+j"
```

Open navigation with the prefix followed by `w`. Use `j`/`k` to move, `h`/`l`
to collapse/expand, `/` to search, and Enter to focus. Numbers identify visible
panes; `Alt+0` selects pane 10. `Alt+j` accepts a multi-digit pane number.

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
