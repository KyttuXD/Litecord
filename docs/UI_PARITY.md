# UI parity screenshots

`litecord --screens <dir>` renders every screen, menu, popup and dialog of the UI to PNG files, with the
same sample data every time. Run it on two operating systems and compare the folders to see where the UI
looks different. A CI workflow does exactly that for Linux and macOS.

## Render the screens

```sh
cargo run --release --bin litecord -- --screens out-linux
```

The run needs no display, network, audio device or account, and it never reads or writes your settings:

- The window is a software-rendered `AppWindow` at 1280x800, scale 1, English, dark theme (a few screens
  switch to the light theme on purpose and say so in their name).
- A tiny Slint platform with a fake clock drives it, so animations are finished when a picture is taken and
  two runs on the same machine give identical files.
- The process switches into `<dir>/.data` before anything else, so files the app writes with a relative path
  land there. The gateway, tray, audio, hardware monitor and updater are never started.

Output:

- `NN-name.png`, one per screen, numbered in the order they are taken.
- `index.json`, listing every picture with its number, file name, short name, a one-line title and size.

The code is in `src/screens.rs`. It fills the window through the same setters the real code uses
(`set_guilds`, `set_messages`, `set_show_settings_modal`, ...). The data loaders and caches are not used, so
the sample data does not depend on downloaded avatars, emoji or attachments.

## Which screens

| Group | Screens |
| --- | --- |
| Login | token panel with and without a QR code, QR scanned, error banner, information banner |
| Chat | server chat (replies, links, commands, custom emoji, bot tag), embed with every button style, code block, image / not yet downloaded image / file attachments, invite and event cards, load older messages, slash command list, command parameter box, typed text, title bar error, hardware monitor off, voice banner (connecting, connected, muted, deafened), web share error toast, wide sidebar |
| Direct messages | conversation list with unread badges, badge above 99, incoming call panel, call in progress |
| Members, profile, friends | member list collapsed and loading (the default chat pictures show it filled), threads and a forum under their channels, profile card (loading, full, with a status line, own profile), server menu (plain, notice, confirm leave), account menu with a status notice, friends view (online, all, pending, blocked, empty, add by username ok and error) |
| Voice room | connecting, participants, own mute, watching a stream, focused stream, own stream (with reduced preview notice), web share states |
| Dialogs | web share dialog (link, copied), account menu (with and without accounts), update dialog (offer, downloading) |
| Share your screen | screens (three quality settings), windows, cameras, no camera |
| Settings | voice and audio (top, lower options, microphone test, output / input / noise suppression dropdowns), video and interface (dropdowns, notice off, capture status), keybinds (idle, recording), appearance and language, network and proxy (modes, test states, diagnostics verdicts, mode dropdown), privacy and about (idle, checking, up to date) |
| Light theme | login, chat, account menu, two settings pages |
| Detached stream window | normal, own stream pinned with the notice |

### Not in the list

- A two-factor code step: the login screen has no such step yet, the app only offers the token and QR
  paths. Add a screen when the UI gets one.
- Reactions, spoilers and stickers on messages: `ChatMessage` has no fields for them yet, so there is
  nothing to render. Add them to `sample_messages()` in `src/screens.rs` together with the UI.
- Operating system menus (tray icon menu, native window frame, file pickers): they are not Slint
  elements.

## Compare two runs

```sh
python3 scripts/ui_diff.py out-linux out-macos --out ui-diff
```

(needs `pip install pillow`)

For each picture present in both folders the script prints the share of pixels that differ, writes
`ui-diff/diff-<name>.png` (the first picture dimmed, changed pixels in red) and a table in
`ui-diff/report.md`. A picture present on one side only is listed as such and makes the exit status 1.
Differences alone never fail the run; add `--fail-above 2` to fail above a percentage.
`--tolerance` (default 8) is the per-channel step still counted as equal, which absorbs rounding noise.

Some difference is expected: the operating system supplies the fallback fonts for characters the bundled
Inter font lacks (arrows, check marks, CJK), so text in those places differs. A layout shift, a missing
icon or a different colour is what to look for.

## CI

`.github/workflows/ui-parity.yml` runs on pushes to `feature/**` and on demand:

1. `linux` renders `out-linux` under `xvfb-run`.
2. `macos` renders `out-macos` on `macos-15-intel`, with the same environment as the macOS compile check.
3. `compare` downloads both, runs `scripts/ui_diff.py`, appends `report.md` to the job summary and
   uploads `ui-diff/` (report and diff images) as the `ui-parity-report` artifact.

The pictures themselves are the `screens-linux` and `screens-macos` artifacts.

## Adding a screen

Add a `self.shot("name", "one line title")?` call in the matching group of `src/screens.rs`, after setting
the properties that screen needs. Reset them afterwards so the next screen starts clean. Dropdowns are
opened by clicking at a fixed position of the 1280x800 layout (`click`, `scroll`); if a settings page is
rearranged, check that the dropdown pictures still show an open list and adjust the coordinates.

## Limits of the pictures

The screenshots are rendered with the Slint software renderer, not the femtovg GPU path of the shipped app. They show layout and content parity, not pixel output. Fonts come from the OS font set, so text differences between Linux and macOS may come from the OS and not from the UI. Consider a higher `--tolerance` for text-heavy screens.
