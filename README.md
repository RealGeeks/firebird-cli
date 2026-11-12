# firebird-cli

The Firebird command line. `firebird-cli` reads and edits a Firebird website
through the Firebird API. It is built to be driven by scripts and AI agents as
much as by people: JSON on stdout when piped, JSON errors on stderr, stable
exit codes, and a built-in guide written for agents.

**Status:** pre-1.0. What exists today: signing in with an API key; reading
pages, blocks, drafts, versions, navigation, theme, footer, headers, social
links, areas, media and revisions; editing a page's draft (add, patch, move,
duplicate, remove and reorder blocks, then validate, publish or discard);
and changing what is live at once: the theme, footer, headers, social links,
page settings and featured areas, restoring a revision, and importing,
uploading and describing media. Saving navigation has no command yet and
goes through the raw `firebird-cli api`; a dedicated command is coming.
Expect breaking changes.

```bash
pbpaste | firebird-cli auth login --site www.example.com --api-key-stdin
firebird-cli me
firebird-cli pages content about
firebird-cli blocks patch about <block> --set title="Welcome home"
firebird-cli draft validate about
firebird-cli draft publish about --message "New headline"
```

## Install

macOS and Linux:

<!-- x-release-please-start-version -->
```bash
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/realgeeks/firebird-cli/v0.1.0/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/realgeeks/firebird-cli/v0.1.0/install.ps1 | iex
```
<!-- x-release-please-end -->

These install the latest release from the
[Releases page](https://github.com/RealGeeks/firebird-cli/releases).

The scripts download the archive for your platform (macOS on Apple silicon
and Intel, Linux x86_64 and arm64 as static binaries, Windows x86_64), check
it against the `.sha256` published beside it and, when the
[GitHub CLI](https://cli.github.com/) is installed and logged in, verify its
build-provenance attestation; without `gh` they print the command to check
it yourself. `install.sh` installs to `/usr/local/bin` if you can write
there, else `~/.local/bin`; `install.ps1` to
`%LOCALAPPDATA%\Programs\firebird-cli`, which it adds to your PATH.

| Variable                         | Flag (`install.sh` / `install.ps1`) | Meaning                                    |
| -------------------------------- | ----------------------------------- | ------------------------------------------ |
| `FIREBIRD_CLI_VERSION`           | `--version` / `-Version`            | Install this tag instead of the latest     |
| `FIREBIRD_CLI_INSTALL_DIR`       | `--dir` / `-InstallDir`             | Where the binary goes                      |
| `FIREBIRD_CLI_SKIP_PROVENANCE=1` |                                     | Skip the attestation check (sha256 only)   |
| `GH_TOKEN` / `GITHUB_TOKEN`      |                                     | Avoids GitHub's anonymous API rate limit   |

Piped into `sh`, flags go after `-s --`:
`curl … | sh -s -- --version v0.1.0`. <!-- x-release-please-version -->

### Updating

```bash
firebird-cli update            # install the latest release over this binary
firebird-cli update --check    # only report whether one is available
firebird-cli update --tag v0.1.0   # x-release-please-version
```

`update` downloads the same archives as the install scripts, checks the
sha256, and verifies the archive's build-provenance attestation itself (no
`gh` needed) before replacing the binary: it must have been built by this
repository's release workflow on `main`. Nothing updates on its own. On a
terminal, a command mentions a newer release at most once a day;
`FIREBIRD_CLI_NO_UPDATE_CHECK=1` turns that off, and piped output and CI
never get it. The updater talks only to GitHub and is never given your site's
API key or token.

### Build from source

With a Rust toolchain (1.88 or newer):

```bash
git clone https://github.com/RealGeeks/firebird-cli
cd firebird-cli
cargo install --path .
firebird-cli --version
```

## Authentication

Firebird uses API keys only; there is no browser login. A site's owner
creates a "client integration" key on the site's **Integrations** page in the
admin. Production keys start `fb_live_`, keys for other environments
`fb_test_`.

```bash
# read the key from stdin so it stays out of shell history and the process list
pbpaste | firebird-cli auth login --site www.example.com --api-key-stdin

firebird-cli me            # the site, the key's prefix and what its token says
firebird-cli auth sites    # the stored sites
firebird-cli auth use www.other.com
firebird-cli auth logout
```

- `auth login` checks the key with the API before saving it. A key the API
  refuses is exit 3 and nothing is stored. `--api-key <KEY>` also works, with
  a warning, since it leaves the key in shell history.
- `--site` is a label for the stored key. The API works out the site from the
  key itself. The first stored site becomes the default.
- Keys are stored in `~/.config/firebird-cli/config.toml` (mode 600, directory
  700; `XDG_CONFIG_HOME` is honoured).
- The API is a separate host from the site, `https://firebird-api.witco.net`
  by default. For a key from another environment, log in with
  `--base-url https://api.staging.example.com` (stored with the key), or
  `--base-url http://localhost:9006` for a local API.
- The CLI exchanges the key for a 15-minute token, keeps it in `tokens.json`
  next to the config file (mode 600) so the next command reuses it, replaces
  it a minute before it expires, and exchanges once more if the API answers
  401.
- The key and token are only sent to the configured API origin, over https
  (plain http only for localhost and other local dev hosts), never across a
  redirect to another origin, and are never printed, `-v` included.
- `firebird-cli me` comes from the token: the API has no "who am I" endpoint,
  so the CLI exchanges the key and shows the token's `siteUuid`, `userUuid`,
  `clientRole` and expiry, with the site label, API and the first 12
  characters of the key. The token is read, not verified.

## Environment variables

| Variable                    | Same as              | Meaning                                         |
| --------------------------- | -------------------- | ----------------------------------------------- |
| `FIREBIRD_SITE`             | `--site`             | Site domain a key is stored under               |
| `FIREBIRD_API_KEY`          | `--api-key`          | API key; overrides the stored one               |
| `FIREBIRD_BASE_URL`         | `--base-url`         | API origin                                      |
| `FIREBIRD_CONFIG_DIR`       |                      | Directory for `config.toml` and `tokens.json`   |
| `FIREBIRD_ALLOW_ANY_SOURCE` | `--allow-any-source` | `1` to let `-d @file` read hidden paths         |

`FIREBIRD_API_KEY` alone is enough to run a command; no site needs to be
stored.

## Commands

```
firebird-cli auth login --site <domain> [--api-key-stdin | --api-key <KEY>] [--base-url URL]
firebird-cli auth logout | status | sites | use <domain>
firebird-cli me

firebird-cli pages list | get <page> | content <page>
firebird-cli pages create --slug SLUG [--meta-title ..] [--meta-description ..] [--meta-keywords ..]
firebird-cli pages update <page> [--slug ..] [--meta-title ..] [--meta-description ..]
                                 [--meta-keywords ..] [--status active|unpublished]
firebird-cli blocks types | schema <codeName>
firebird-cli blocks add <page> <codeName> [--position N] [--data JSON|@file|-] [--set k=v]...
firebird-cli blocks patch <page> <block> [--data JSON|@file|-] [--set k=v]...
firebird-cli blocks move <page> <block> up|down|top|bottom
firebird-cli blocks duplicate <page> <block>
firebird-cli blocks remove <page> <block>
firebird-cli blocks order <page> <block>...
firebird-cli draft list | status <page> | validate <page> | discard <page>
firebird-cli draft publish <page> [--message TEXT]
firebird-cli versions list <page> | get <page> <n> | restore <page> <n>

firebird-cli nav get [--nav-bar UUID | --page <page>] | anchors <page>
firebird-cli styling get | fonts | set [k=v]... [--data JSON|@file|-]
firebird-cli footer get | set [k=v]... [--data ...]
firebird-cli headers list | get <headerUuid> | update <headerUuid> [k=v]... [--data ...]
firebird-cli social get | set [platform=handle]... [--data ...]
firebird-cli areas list | create --name .. | update <areaUuid> | delete <areaUuid>
                 [--description ..] [--media-uuid ..] [--route-location ..] [--custom-url ..]
                 [--lat N] [--lon N]
firebird-cli media list [--query TEXT] [--tags a,b] [--media-type T] [--asset-role R]
                        [--orientation O] [--limit N] [--offset N] | get <mediaUuid>
firebird-cli media import <url> [--alt TEXT]
firebird-cli media upload <file> [--alt TEXT] [--mime-type TYPE] [--filename NAME]
firebird-cli media update <mediaUuid> [--alt ..] [--description ..] [--tags a,b] [--asset-role R]
firebird-cli revisions list --type <resourceType> [--key <resourceKey>] [--change-set UUID]
                            [--cursor N] [--limit N] | get <revisionUuid>
firebird-cli revisions restore <revisionUuid> [--message TEXT]

firebird-cli api METHOD PATH [-p k=v]... [-d JSON | -d @file | -d -] [--no-auth]
firebird-cli guide [topic] [--list]
firebird-cli update [--check] [--tag vX.Y.Z]
firebird-cli completions <shell>
```

`<page>` is a page's uuid or its slug: `about`, `/about/`, and `/` or `home`
for the home page. `<block>` is a block's uuid, live or draft.

Global options: `-o json|jsonl|table` (or `--json`), `-q` for identifiers
only, `-y` to skip a confirmation, `-v` to log requests, `--max-retries N`
for 429s. Every command has `--help`.

Editing works on a page's draft; visitors see nothing until it is published:

```bash
firebird-cli blocks schema info                       # the fields a block type takes
firebird-cli blocks add about info --set title="Our story"
firebird-cli blocks patch about $BLOCK --set button.text=Go --set subtitle=null
firebird-cli draft validate about                     # exit 5 if it cannot be published
firebird-cli draft publish about -y --message "Add our story"
firebird-cli draft discard about -y                   # or throw the draft away
```

- `blocks patch` sends a merge patch: only the fields given change. `--set`
  values are JSON when they parse as JSON and text otherwise, dotted keys
  nest, and `--set` wins over the same field in `--data`.
- `draft publish`, `draft discard`, `blocks remove` and `versions restore`
  ask for confirmation on a terminal; `-y` skips the question. A script or
  agent (no terminal) is not asked and the command runs.
- A block write the API refuses can still leave a new draft and the page's
  edit lock behind; `draft status <page>` shows it and `draft discard`
  removes it.

Output is the API's JSON unchanged when piped, and a table on a terminal.
Errors are one JSON line on stderr in JSON mode, with the API's whole error
response under `error.body`:

```json
{"error":{"code":"not_found","message":"Page not found","status":404,"body":{"error":"Page not found"},"exit_code":4}}
```

Settings are not drafts: they change the live site the moment a command
succeeds.

```bash
firebird-cli styling set spacing=compact primaryColor=#1a4d8f
firebird-cli footer set phone="555-0100" isRealtor=false
firebird-cli pages update about --meta-title "About our team"
firebird-cli revisions list --type styling
firebird-cli revisions restore $REVISION        # put a setting back
firebird-cli media upload ./porch.jpg --alt "A front porch"
```

- `styling set`, `social set`, `footer set`, `headers update` and
  `pages update` change only the fields you pass. The API replaces the whole
  object on a save, so the CLI reads the current value, applies your fields
  and sends everything back; two writers doing that at once can undo each
  other.
- The theme, social links, footer and headers keep a history: a save prints
  its `revisionUuid`, and `revisions restore` of the revision before it
  undoes it. Page settings, featured areas and media details keep no
  history.
- `media upload` accepts JPEG, PNG, WebP, GIF and AVIF images up to 20 MB
  and MP4, WebM and QuickTime videos up to 75 MB. The bytes go to a storage
  address the API hands out; your key and token are never sent there.
- The commands that replace or delete ask for confirmation on a terminal;
  `-y` skips the question, and a script or agent is not asked.

`api` calls any endpoint with the stored key, for what has no command yet
(saving navigation). `-d @file` refuses hidden files and credential folders,
because what it reads is sent to the API:

```bash
firebird-cli api PUT /navigation -d @navigation.json
firebird-cli api GET /docs --no-auth
firebird-cli api GET /healthz/ready --no-auth
```

`firebird-cli guide` prints the manual for scripts and agents (also at
[docs/GUIDE.md](docs/GUIDE.md)): what is live and what waits in a draft, how
block data is written, undo, and the content rules. `guide --list` shows its
topics and `guide <topic>` prints one. [AGENTS.md](AGENTS.md) has the short
version.

## Exit codes

| Code | Meaning                                                              |
| ---- | -------------------------------------------------------------------- |
| 0    | Success                                                              |
| 1    | Any other failure, including a 5xx from the API                      |
| 2    | Usage: bad flags or arguments, invalid JSON, a refused file or URL, a confirmation not given |
| 3    | Authentication: no key, a refused key (401), or not allowed (403)    |
| 4    | Not found (404)                                                      |
| 5    | Validation: the API refused the request (400, 413, 415, 422), or `draft validate` found errors |
| 6    | Conflict (409)                                                       |
| 7    | Rate limited (429) after every retry                                 |
| 8    | Network: the API could not be reached, timed out, or TLS failed      |
| 9    | The API requires a newer firebird-cli (426): run `firebird-cli update` |

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

All four must pass clean. The tests run the binary against a mock API, so
they need no Firebird site or key. See [AGENTS.md](AGENTS.md) for the layout
and conventions, [CONTRIBUTING.md](CONTRIBUTING.md) for pull requests and how
releases are made, and [SECURITY.md](SECURITY.md) to report a vulnerability
or verify a download.

## License

[MIT](LICENSE)
