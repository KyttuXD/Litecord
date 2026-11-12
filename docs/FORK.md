# This fork and how it stays in sync with upstream

Upstream is [Ak4ai/Litecord](https://github.com/Ak4ai/Litecord). This fork adds the macOS Intel build
([MACOS.md](MACOS.md)) and one release workflow, and tries to touch as little of upstream as possible so
that new upstream versions keep merging without conflicts.

## Rules for changes made here

- **New things go in new files**: `.github/workflows/nightly.yml`, `docs/MACOS.md`, `docs/FORK.md`,
  `packaging/macos/`, `scripts/macos/`, `src/screen_capture/sources/macos.rs`.
- **Upstream files are not rewritten**: `README.md`, `CHANGELOG.md`, the wiki pages and upstream's
  workflows (`build.yml`, `ci.yml`) are left exactly as upstream has them.
- Where an upstream source file must change, the change is a small `#[cfg(target_os = "macos")]` block
  next to the existing Windows/Linux ones, never a rewrite of the surrounding code.

## Releases

`nightly.yml` is the only workflow that publishes here. Every night (and on demand from the Actions tab)
it merges upstream into `main` when GitHub allows it, builds Windows, Linux, the AppImage and macOS Intel,
and replaces the single `nightly` pre-release. Upstream's `Build & Release Litecord` workflow is disabled
in this repository's Actions settings (a setting, not a file change), and upstream's version tags are not
kept here.

## Syncing by hand

The nightly sync is refused when upstream changed a workflow file or when the merge conflicts. Then:

```bash
git fetch origin
```

```bash
git merge origin/main
```

```bash
git push fork main
```

(`origin` is upstream, `fork` is this repository.) Conflicts can only be in the few source files listed in
[MACOS.md](MACOS.md); keep both sides, the macOS blocks are independent of the code around them.
