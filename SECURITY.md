# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub: on this repository's
**Security** tab choose **Report a vulnerability**, or go straight to
<https://github.com/RealGeeks/firebird-cli/security/advisories/new>. Do not
open a public issue, pull request or discussion for a security problem.

Useful details: the firebird-cli version (`firebird-cli --version`), your
operating system, the command involved, and what an attacker could do. Never
include a real API key; revoke any key that may have been exposed on the
site's **Integrations** page.

We will acknowledge the report, keep you updated while we work on a fix, and
credit you in the advisory if you would like.

## Supported versions

Security fixes go into the latest release. Update with `firebird-cli update`,
the install command in the [README](README.md#install), or by downloading
the newest [release](https://github.com/RealGeeks/firebird-cli/releases).

## Scope

This repository covers the `firebird-cli` client and its install scripts.
Problems with a Firebird website or the Firebird API itself can be reported
the same way and will be passed to the right team.

## Verifying a download

Every release archive has a `.sha256` checksum beside it and a signed
build-provenance attestation. `firebird-cli update` verifies the attestation
itself (Sigstore's trust root is built in) and refuses an archive that was
not built by this repository's release workflow on `main`; the install
scripts do the same through the GitHub CLI when it is installed and logged
in, and otherwise print the command. To check an archive by hand:

```bash
gh attestation verify firebird-cli-<version>-<target>.tar.gz --repo RealGeeks/firebird-cli \
  --cert-identity https://github.com/RealGeeks/firebird-cli/.github/workflows/release.yml@refs/heads/main
```

`FIREBIRD_CLI_SKIP_PROVENANCE=1` skips the attestation in both, for mirrors
that cannot reach GitHub; the sha256 is still checked.

## Stored keys and tokens

API keys are kept in `~/.config/firebird-cli/config.toml`, and the
short-lived tokens they are exchanged for in `tokens.json` beside it. Both
are readable only by you (mode 600 in a 700 directory, restored with a
warning if found looser). They are not in the operating system's keychain;
that is a possible future option. Anyone who can run code as your user can
read the files, so revoke a key on the site's **Integrations** page if the
machine may be compromised.

The key and token are sent only to the configured Firebird API origin, over
https. `firebird-cli update` and the install scripts talk to GitHub and are
never given either.
