# Releasing lyrfin

How a lyrfin release is cut, why it works that way, and how the automation is
wired. This is the reference for future-me. For the policy layer above these
mechanics — the one-decision workflow and what never to ask — see
[`RELEASE_MANAGER.md`](RELEASE_MANAGER.md).

## Two paths

**Automated (default) — release-please.** Every push to `main` updates a standing
"release PR" that bumps the version (`Cargo.toml` + `Cargo.lock`) and rewrites
`CHANGELOG.md` from the Conventional Commits since the last release. When you are
ready to ship, review and **merge that PR**: release-please tags `vX.Y.Z`,
publishes the GitHub Release with generated notes, and the binaries build and
attach automatically.

```
# Day-to-day: just land Conventional Commits on main — nothing else to do.
# To ship:  open the "chore: release X.Y.Z" PR that release-please maintains,
#           sanity-check the version + CHANGELOG it computed, and merge it.
```

**Manual (fallback / hotfix) — tag push.** Push a `vX.Y.Z` tag by hand;
`release.yml` first requires CI to be green on that commit, then creates the
GitHub Release from the matching `CHANGELOG.md` section and builds the binaries.

```sh
git switch -c release/X.Y.Z
$EDITOR Cargo.toml CHANGELOG.md          # bump version; write the dated section
cargo check                              # sync Cargo.lock's lyrfin version
git commit -am "chore(release): vX.Y.Z"
git switch main && git merge --no-ff release/X.Y.Z && git branch -d release/X.Y.Z
git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin main vX.Y.Z
```

## Versioning (SemVer)

lyrfin follows [Semantic Versioning](https://semver.org). release-please computes
the next version from the Conventional Commit types since the last `vX.Y.Z` tag:

| Change | Bump | Example |
|--------|------|---------|
| New user-facing feature (`feat:`) | **minor** | `0.2.0 → 0.3.0` |
| Bug fix / polish (`fix:`) | **patch** | `0.2.0 → 0.2.1` |
| Breaking change (`!` / `BREAKING CHANGE`) | minor while 0.x | noted in the changelog |
| `perf`, `refactor`, `docs`, `chore`, `ci`, `test`, `build`, `style` | **no release** | (shown in changelog only if `feat`/`fix` also landed) |

`feat` bumps the **minor** even pre-1.0 because the config sets
`bump-minor-pre-major: true` (`bump-patch-for-minor-pre-major: false`) — see
`release-please-config.json`.

## The changelog

`CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com).
**release-please writes it for you** from the commits — you only review the
proposed entry in the release PR. For the manual path, curate it by hand from the
Conventional Commit subjects since the last tag
(`git log vLAST..HEAD --no-merges --pretty=format:'%s'`).

## How the automation is wired

- **`.github/workflows/ci.yml`** — on every push to `main` and on PRs: build,
  test, a headless `--snapshot` smoke test, `cargo fmt --check` and
  `cargo clippy -D warnings`, across Linux/macOS/Windows.
- **`.github/workflows/pr-title.yml`** — on PRs: validates the PR *title* is a
  Conventional Commit. Because the repo squash-merges with the title as the commit
  subject, that title is the single commit release-please reads — the guard against
  a malformed title silently dropping a change from the release.
- **`.github/workflows/release-please.yml`** — on push to `main`: maintains the
  release PR and, when it merges, tags `vX.Y.Z` + creates the GitHub Release. It
  authenticates with a **GitHub App token** (the `dilmun-release-bot` App), *not*
  `GITHUB_TOKEN`, so (a) the release PR it opens triggers CI + the PR-title check
  like any human PR — otherwise those *required* checks never run and branch
  protection (`enforce_admins: true`) makes the PR un-mergeable — and (b) the
  `vX.Y.Z` tag it creates triggers `release.yml`, which does the build. There is
  **no build job here** (that would double-build). App creds are the secrets
  **`RELEASE_APP_CLIENT_ID`** + **`RELEASE_APP_PRIVATE_KEY`**; the App needs
  **contents** + **pull-requests** write (+ **issues** for PR labels).
- **`.github/workflows/release.yml`** — the build-and-publish path for **every**
  `v*` tag, automated or manual. It **waits for CI to conclude `success` on that
  commit**, then *ensures* a GitHub Release exists (release-please already made one
  with generated notes on the automated path — those are kept; a manual tag gets
  one from the matching `CHANGELOG.md` section), then calls `release-build.yml`. It
  also accepts a **`workflow_dispatch`** with a `tag` input to **rebuild + re-attach
  the binaries for an existing release** — the recovery path when one target flakes
  after the release is already cut (uploads `--clobber`, so it's idempotent and
  needs no tag surgery). Run it from the Actions tab or
  `gh workflow run release.yml -f tag=vX.Y.Z`.
- **`.github/workflows/release-build.yml`** — reusable (`workflow_call`, input
  `tag`): builds the four target binaries (Linux x86-64, macOS Intel + Apple
  Silicon, Windows x86-64) and attaches each to the release alongside a `.sha256`
  checksum. The checksum step prefers `shasum` and falls back to `sha256sum`, since
  macOS ships only the former and the Windows Git Bash runner only the latter — a
  target-specific gap that silently broke Windows packaging until it was fixed.
  Verify a download with `shasum -a 256 -c lyrfin-<target>.tar.gz.sha256`.

Config lives in **`release-please-config.json`** (`release-type: rust`; tags as
`vX.Y.Z` via `include-component-in-tag: false`; `feat`→minor pre-1.0) and
**`.release-please-manifest.json`** (the current released version).

## Why release-please (not release-plz)

lyrfin is a **GitHub-only application**, never published to crates.io. release-plz is
built around publishing crates: it queries the registry, runs `cargo package`
(which requires every git/path dependency to declare a `version` — lyrfin's
`librespot` git dep does not), and derives SemVer from `cargo-semver-checks` (which
does nothing for a binary, defaulting to a patch bump in 0.x). release-please is
purely Conventional-Commit-based, never runs cargo, needs no registry, and bumps
`feat`→minor pre-1.0 with one config flag — a clean fit for this repo. (Switched
back from a brief release-plz trial on 2026-07-09.)

## Pre-release checklist (manual path only)

- [ ] `main` CI is green.
- [ ] `Cargo.toml` version bumped; `Cargo.lock` `lyrfin` entry matches
      (run `cargo check` after editing the manifest).
- [ ] `CHANGELOG.md` has the new dated section.
- [ ] Version in the changelog == the tag you are about to push.
