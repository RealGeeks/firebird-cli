# firebird-cli

The Firebird command line. `firebird-cli` reads and edits a Firebird website
through the Firebird API. It is built to be driven by scripts and AI agents as
much as by people: JSON on stdout when piped, JSON errors on stderr, stable
exit codes, and a built-in guide written for agents.

**Status:** pre-1.0, phase 1. What exists today is signing in with an API
key, four read commands (pages, a page's blocks, block types, a block type's
schema), a raw `api` command that reaches every other endpoint, and the
guide. Dedicated commands for editing, publishing, settings and media are
planned; until then they go through `firebird-cli api`. Expect breaking
changes.

```bash
pbpaste | firebird-cli auth login --site www.example.com --api-key-stdin
firebird-cli me
firebird-cli pages list
firebird-cli pages content <pageUuid>
```

## Build from source

There are no release binaries yet. With a Rust toolchain (1.88 or newer):

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
firebird-cli pages list
firebird-cli pages content <pageUuid>
firebird-cli blocks types
firebird-cli blocks schema <codeName>
firebird-cli api METHOD PATH [-p k=v]... [-d JSON | -d @file | -d -] [--no-auth]
firebird-cli guide [topic] [--list]
firebird-cli completions <shell>
```

Global options: `-o json|jsonl|table` (or `--json`), `-q` for identifiers
only, `-v` to log requests, `--max-retries N` for 429s. Every command has
`--help`.

Output is the API's JSON unchanged when piped, and a table on a terminal.
Errors are one JSON line on stderr in JSON mode, with the API's whole error
response under `error.body`:

```json
{"error":{"code":"not_found","message":"Page not found","status":404,"body":{"error":"Page not found"},"exit_code":4}}
```

`api` calls any endpoint with the stored key. `-d @file` refuses hidden files
and credential folders, because what it reads is sent to the API:

```bash
firebird-cli api GET /pages
firebird-cli api PATCH /pages/$PAGE/blocks/$BLOCK/data -d '{"data":{"title":"Welcome home"}}'
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
| 2    | Usage: bad flags or arguments, invalid JSON, a refused file or URL   |
| 3    | Authentication: no key, a refused key (401), or not allowed (403)    |
| 4    | Not found (404)                                                      |
| 5    | Validation: the API refused the request (400, 413, 415, 422)         |
| 6    | Conflict (409)                                                       |
| 7    | Rate limited (429) after every retry                                 |
| 8    | Network: the API could not be reached, timed out, or TLS failed      |
| 9    | The API requires a newer firebird-cli (426)                          |

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

All four must pass clean. The tests run the binary against a mock API, so
they need no Firebird site or key. See [AGENTS.md](AGENTS.md) for the layout
and conventions.

## License

[MIT](LICENSE)
