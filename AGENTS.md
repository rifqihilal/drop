# AGENTS.md

Windows-only native notch app in Rust. One binary, no webview: Win32 windows, Direct2D and DirectWrite for drawing. The UI text is Indonesian.

## Commands

- `cargo build --release` — the running `drop.exe` locks the file, so close Drop first
- `cargo fmt`, `cargo clippy --release --all-targets`, `cargo test --release` — all must pass before committing. CI runs clippy, tests and a format check on `windows-latest`
- `target/release/drop.exe --demo` previews notices, `--settings` opens Settings, `--idle-secs N` overrides the idle delay

## Architecture

- `src/main.rs` — `App`: window procedure, tick loop, page list, click handling, and the `notices` module
- `src/anim.rs` — spring physics and every notch and card size
- `src/render.rs` — layered window rendering: `Scene` is what one frame shows, `draw_card_header` frames each card, `draw_*_card` draws a page
- `src/settings.rs` — the settings window (a separate normal window)
- `src/config.rs` — `drop.ini`, read and written by hand-rolled code so the file stays editable
- Worker threads post results to the notch window as `WM_APP + n` messages with a boxed payload: `weather.rs`, `ical.rs`, `lyrics.rs`, `media.rs`, `geo.rs`. Check for free numbers before adding one
- System events: `system.rs` (audio, power, battery, full-screen, clipboard), `wifi.rs`, `bluetooth.rs`, `keys.rs` (volume keys), `osd.rs`, `tray.rs`, `dragdrop.rs`, `frames.rs` (display-paced animation)
- Pure logic with tests: `prayer.rs`, `calendar.rs`, `health.rs`, `todo.rs`, `shelf.rs`, and the parsers in `ical.rs` and `lyrics.rs`

## Conventions

- Stay light. The budget is 30 MB of RAM, and idle CPU should stay near zero: event-driven where Windows allows, timers only as slow as they can be
- Pages draw at their own origin. The card frame places them under `CARD_TOP`, between `CARD_SIDE` margins; hit tests convert with `on_page`
- A new card page needs: a `Page` variant, a title in `page_title`, a size in `anim.rs`, its place in `App::pages`, and a switch under Modul in Settings
- A new setting needs: a field in `Settings` and its default, a line in `to_ini`, a key in `parse` (keys are global, sections are only for reading), and the round-trip test
- The `windows` crate is pinned to 0.61 with an explicit feature list in `Cargo.toml`; add features there when using new Win32 APIs
- Commits follow conventional commits (`feat:`, `fix:`)

## Gotchas

- The notch window has `WS_EX_NOACTIVATE`. It takes the keyboard only while a task is typed, and gives it back afterwards
- Explicitly drop `App` before `main` returns; COM objects released during process teardown stall exit by seconds
- Windows 11 draws its volume flyout inside Explorer, so `keys.rs` takes over the volume keys rather than hiding it
- Test with `APPDATA` pointed at a scratch folder so the real settings stay untouched, and keep prayer reminders off there: one covers the card in screenshots
