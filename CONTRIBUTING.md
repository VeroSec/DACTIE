# Contributing

Three rules hold for everything in this repository. CI (`.github/workflows/`) checks them on
every pull request and every push to `main`. Merge a pull request only when all checks
pass.

## 1. Nothing that points at our infrastructure or its people

Not allowed in code, data, commit messages, or pull request titles and descriptions:
- domain names, except public ones listed in `.github/allowed-domains.txt` (code hosting,
  standards, papers, `example.com`);
- public IP addresses;
- user names, including home directories (`/home/<name>`) and `<name>@<host>`;
- e-mail addresses, except `noreply` and `example.com` addresses;
- keys, passwords and tokens, and key, keystore or `.env` files.

Allowed: the host names of the testbed (minerva1–4, labrada) and private IP addresses
(such as `10.244.192.0/24`), because the results and `docs/RUNBOOK.md` refer to them.

How it is checked (`scripts/check_public.py`):
- every tracked file, `.gz` files included;
- in a pull request, the message and every added line of each commit, so that something
  added and removed again is still caught: it stays in the public history;
- the pull request's title and description, again after every edit.

Things to know:
- **CI finds a leak only after it is public.** Install the pre-push hook, which runs the same
  check before anything leaves your machine:
  `ln -s ../../scripts/hooks/pre-push .git/hooks/pre-push`
- **Check a pull request description before you post it:**
  `python3 scripts/check_public.py text pr < description.md`
- **Private deny list.** User names and internal domains that must never appear cannot be
  listed in a public repository. The check reads them from the repository secret
  `PUBLIC_DENYLIST` in CI, and from `~/.config/dactie/denylist` on your machine (one term per
  line).
- **Findings are masked** (`mi****.de`), because CI logs of a public repository are public.
  Run the check locally to see where a finding is.
- **Exceptions.** `workloads/` holds public threat-intelligence records, whose domains and
  addresses are their content; it is checked only for keys and the deny list. A line with the
  marker `public-ok` is treated the same way; use it for test values and say why on the
  same line.

## 2. Code passes the linters

Every file passes its linter, with the versions pinned in `scripts/lint.sh` (Rust: the version
of `rust-toolchain.toml`):

| Code | Linter |
| --- | --- |
| Python | ruff, default rules (`ruff.toml`) |
| Rust | rustfmt, and clippy without warnings |
| Shell scripts | shellcheck, all severities (`.shellcheckrc`) |
| Dockerfiles | hadolint |
| GitHub workflows | actionlint |
| Go | gofmt and go vet |

Run them all with `scripts/lint.sh`, or one with `scripts/lint.sh python|rust|shell|docker|actions|go`;
it needs Docker, and cargo for Rust. CI runs the same script. Automatic fixes: `cargo fmt --all`,
`cargo clippy --fix`, and `ruff check --fix`.

Silence a lint only where the code is right as it is, on that line, with a comment that says why
(`#[allow(clippy::...)]`, `# shellcheck disable=SC...`, `# noqa: ...`).

## 3. Docker builds are checked

CI builds every Dockerfile and validates `docker-compose.yml`:
- `crates/dactie-authority`, `crates/dactie-archive`, `crates/dactie-peer` (also with
  `FEATURES=bench`, as the compose file builds it);
- `experiments/fabric/loadgen` and `experiments/fabric/chaincode`.

A new Dockerfile gets an entry in the `docker` job of `.github/workflows/checks.yml`.

The Rust version is pinned (`rust-toolchain.toml` and `FROM rust:1.100` in the Dockerfiles), so
that a new Rust release cannot break the build unnoticed. Change both together.
