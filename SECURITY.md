# Security Policy

## Reporting a vulnerability

Use GitHub private vulnerability reporting:
<https://github.com/alexeyco/herdr-revdiff/security/advisories/new>.

Do not open a public issue for a security problem. Include a description, the
affected version, and a minimal reproduction if you have one. Expect an initial
response within a week.

## Supported versions

Only the latest tagged release is supported. Fixes land on `master` and ship in
the next release.

## Scope

This policy covers this plugin's own code. It executes two external binaries:

- `HERDR_BIN_PATH`, set by herdr; every callback runs through it.
- `REVDIFF_BIN`, optionally set by you; otherwise the first `revdiff` on `PATH`.

Both binaries run with your privileges, so treat them as trusted input. Report
bugs in herdr or revdiff to their upstream projects — this plugin is not
responsible for their behavior.
