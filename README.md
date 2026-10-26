# tiler

[![CI](https://github.com/maddoxrjohnson/tiler/actions/workflows/ci.yml/badge.svg)](https://github.com/maddoxrjohnson/tiler/actions/workflows/ci.yml)

A keyboard-driven tiling window manager for Windows, inspired by [AeroSpace](https://github.com/nikitabobko/AeroSpace) on macOS.

tiler arranges your windows into an i3-style tree of rows and columns, gives you
instant virtual workspaces, and is driven entirely by keyboard shortcuts or a
command-line tool. Configuration is one plain-text TOML file.

> **Status:** early (pre-1.0). It is in daily use by its author, but expect rough
> edges. If a window ever goes missing, `tiler rescue` brings it back.

## Features

- **Tree layout.** Windows tile in nested rows and columns. Move, join, resize and
  switch containers between *tiles* and *accordion* (overlapping) layouts.
- **Instant workspaces.** tiler has its own workspaces instead of Windows Virtual
  Desktops: switching is immediate, with no animation. Windows on other workspaces
  stay in the taskbar and Alt-Tab, and picking one there switches to its workspace.
- **Multiple monitors.** Each monitor shows one workspace. Move workspaces between
  monitors, pin workspaces to a monitor, and drag a window onto another monitor to
  move it there. Gaps scale with each monitor's DPI.
- **One command language.** Every key binding is a command, and every command
  also works from the terminal: `alt-2 = 'workspace 2'` in the config does the same
  as `tiler workspace 2`.
- **Window rules.** Float, ignore, or send windows to a workspace by process name,
  window class, title, or app name.
- **Plays well with stubborn apps.** Apps with a minimum width (Electron apps, for
  example) get the space they need; the other windows share the rest instead of
  overlapping.
- **Safe to quit or crash.** Hidden windows are restored when tiler exits, when it
  crashes, at logoff, and at the next start if it was killed.

## Install

Download `tiler.exe` from the [latest release](https://github.com/maddoxrjohnson/tiler/releases/latest),
or build it from source:

1. Install Rust: `winget install Rustlang.Rustup`.
2. Build:

   ```powershell
   git clone https://github.com/maddoxrjohnson/tiler
   cd tiler
   cargo build --release
   ```

3. Run `target\release\tiler.exe`. It tiles your open windows and starts listening
   for shortcuts. Stop it with `tiler quit` or Ctrl+C in its terminal.

To start tiler when you log in, set `start-at-login = true` in your config. It then
runs in the background and logs to `%LOCALAPPDATA%\tiler\tiler.log`.

## Default shortcuts

| Keys | Command |
|---|---|
| `alt-h` / `j` / `k` / `l` | Focus the window to the left / below / above / right |
| `alt-shift-h` / `j` / `k` / `l` | Move the focused window in that direction |
| `alt-1` … `alt-9` | Switch to workspace 1 … 9 |
| `alt-shift-1` … `alt-shift-9` | Move the focused window to workspace 1 … 9 |
| `alt-backtick` | Go back to the previous workspace |
| `alt-shift-backtick` | Move the current workspace to the next monitor |
| `alt-slash` | Cycle the layout: tiles, horizontal, vertical |
| `alt-comma` | Cycle the layout: accordion, horizontal, vertical |
| `alt-f` | Toggle fullscreen |
| `alt-shift-space` | Toggle floating |
| `alt-minus` / `alt-equal` | Shrink / grow the focused window |
| `alt-r` | Resize mode: `h` `j` `k` `l` resize, `b` balances, `esc` exits |
| `alt-shift-semicolon` | Service mode: `esc` reloads the config, `r` flattens the tree, `f` toggles floating, `backspace` closes the window |

`alt-tab` is left to Windows. Note that while tiler runs, Alt shortcuts that are
bound (such as `alt-f`) no longer reach applications; change the modifier in your
config if you rely on them.

## Configuration

tiler reads `%USERPROFILE%\.tiler.toml`, and reloads it automatically when you save
it. Without a file, it uses the [built-in defaults](crates/tiler-core/src/default-config.toml);
copy that file as a starting point. Your file replaces the defaults entirely.

```toml
start-at-login = false

[gaps]
inner.horizontal = 8
inner.vertical = 8
outer = 8                 # or { left = 8, top = 8, right = 8, bottom = 8 }

[mode.main.binding]
alt-h = 'focus left'
alt-1 = 'workspace 1'
alt-r = 'mode resize'

[mode.resize.binding]
h = 'resize width -50'
esc = 'mode main'

# Send Slack to workspace 9 and float the calculator.
[[on-window-detected]]
if.process = 'slack.exe'
run = 'move-node-to-workspace 9'

[[on-window-detected]]
if.app-name-regex = '^CalculatorApp$'
run = 'layout floating'

# Keep workspace 9 on the secondary monitor.
[workspace-to-monitor-force-assignment]
9 = 'secondary'
```

Key names are `a`–`z`, `0`–`9`, `f1`–`f24`, arrows (`left`, …), `esc`, `enter`,
`tab`, `space`, `backspace`, and punctuation by name (`minus`, `equal`, `comma`,
`period`, `slash`, `semicolon`, `quote`, `backtick`, …). Modifiers are `alt`,
`ctrl`, `shift` and `win`. Mistakes are reported with the exact key or rule at fault.

## Command line

Any command works from a terminal while the server runs:

```text
tiler workspace 2               tiler move-node-to-workspace --focus-follows-window 3
tiler focus left                tiler layout floating tiling
tiler list-windows              tiler list-workspaces --focused
tiler reload-config             tiler quit
tiler rescue                    # also works when the server is not running
```

Commands: `focus`, `move`, `join-with`, `layout`, `resize`, `balance-sizes`,
`flatten-workspace-tree`, `close`, `fullscreen`, `workspace`,
`workspace-back-and-forth`, `move-node-to-workspace`, `move-workspace-to-monitor`,
`focus-monitor`, `mode`, `reload-config`, `rescue`, `list-windows`,
`list-workspaces`, `list-monitors`, `debug-windows`, `quit`. A typo gets a
suggestion: `unknown command 'focs'; did you mean 'focus'?`

## How it works

```text
 Win32 events ─┐                      ┌─▶ SetWindowPos / hide / focus
 hotkeys ──────┼─▶  tiler-core  ──────┤
 CLI (pipe) ───┘   State::handle(Event) -> Vec<Action>
```

- **`tiler-core`** is pure Rust with no Windows APIs: the layout tree, workspaces,
  command parser, config, and the state machine. It receives events ("window
  appeared", "hotkey pressed", …) and returns the actions to take. It only emits
  the difference between where windows are and where they should be, so tiler
  moves as few windows as possible. Because it never touches the OS, it is tested
  on any platform, including property tests that throw random event sequences at it.
- **`tiler-windows`** is the Win32 layer: window events (`SetWinEventHook`), a
  low-level keyboard hook, monitors and DPI, and hiding windows by moving them past
  the bottom-right corner of the desktop.
- **`tiler`** is the binary: the server loop, the named-pipe CLI, config reloading,
  and crash recovery.

## Limitations

- Windows of apps running as administrator are left alone, because Windows doesn't
  let a normal process move them. Run tiler as administrator to manage them too.
- Key names follow the US keyboard layout.
- Animations, window borders, and drag-to-swap aren't implemented yet.

## Development

```powershell
cargo test --workspace                                # all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo run -- --dry-run                                # log what tiler would do, move nothing
```

`tiler-core` builds and tests on Linux and macOS as well. CI runs formatting,
Clippy and tests on Ubuntu and Windows.

Before a release, the manual smoke test is: tile three windows; switch workspaces
and back; Alt-Tab to a window on a hidden workspace; resize with the mouse and the
keyboard; float and unfloat a dialog; kill the server and run `tiler rescue`; quit.

## License

[MIT](LICENSE)
