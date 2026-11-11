# Contributing to firebird-cli

Thanks for helping. Bug reports, fixes and new commands for Firebird API
routes are all welcome.

- **Security problems** go through [private reporting](SECURITY.md), never a
  public issue.

## Development

You need a Rust toolchain (1.88 or newer).

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings   # pedantic; unwrap/expect/panic are denied
cargo fmt
```

All four must pass before a pull request is merged; CI runs them on Linux,
macOS and Windows, along with `cargo deny` (advisories and licenses), an MSRV
check, a static Linux build and a lint of the GitHub Actions workflows. Tests
run against mock servers; nothing in the test suite talks to a real site or
to GitHub. Point the CLI at a local API with
`FIREBIRD_BASE_URL=http://localhost:9006`.

[AGENTS.md](AGENTS.md) describes the layout and the conventions the code
follows, for people and coding agents alike. In short:

- Field names and flags mirror the Firebird API's names exactly.
- stdout is the result document, the API's response unchanged; anything for
  humans goes to stderr.
- Lints are never silenced with `#[allow]`, in tests either.
- A new command gets a section (or a line in one) in `docs/GUIDE.md` and an
  integration test.

## Pull requests

- Use a [conventional commit](https://www.conventionalcommits.org/) title
  (`feat: …`, `fix: …`, `docs: …`): pull requests are squash-merged under
  their title, and release notes are generated from those commits.
- Keep API keys, customer data, private site content and internal hostnames
  out of code, tests, fixtures and screenshots.
- By contributing you agree that your contribution is licensed under the
  project's [MIT license](LICENSE).

## Releases

Releases are cut by [release-please](https://github.com/googleapis/release-please)
in `.github/workflows/release.yml`; nobody tags or edits a version by hand.

1. Every push to `main` updates an open pull request titled
   `chore(main): release X.Y.Z`. It bumps the version in `Cargo.toml`,
   `Cargo.lock`, the install commands in the docs and the install scripts,
   and writes `CHANGELOG.md` from the commits since the last release.
   Before 1.0, a `feat:` or a breaking change bumps the minor version and a
   `fix:` the patch version.
2. Merging that pull request creates the tag and the GitHub release. The
   same workflow run then builds the archives for macOS, Linux and Windows,
   attests their build provenance, uploads them with their `.sha256` files,
   and runs `install.sh` and `install.ps1` against the new release.
3. If the build fails after the tag exists, fix `main` if needed and re-run
   it for that tag from the Actions tab: **Release → Run workflow**, on
   `main`, with the tag. It must be dispatched from `main`: the attestation
   is only trusted when it names `release.yml@refs/heads/main`.

To force a particular version, put `Release-As: X.Y.Z` in the body of a
commit on `main`.
