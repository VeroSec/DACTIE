#!/usr/bin/env python3
"""Rule 1 of CONTRIBUTING.md: nothing public may point at our infrastructure or its people.

Flags domain names (except the public ones in .github/allowed-domains.txt), public IP
addresses, home directories, e-mail addresses, private keys, passwords, key files, and the
terms of a private deny list. Host names (minerva1-4, labrada) and private IP addresses
are allowed, because the results and the runbook refer to them.

Usage:
  check_public.py tree               every tracked file (.gz files are decompressed)
  check_public.py commits REV...     messages and added lines of the commits that
                                     `git rev-list REV...` lists, e.g. main..HEAD
  check_public.py text NAME          text from stdin, e.g. a PR title or body

Exceptions:
  - Third-party data (workloads/: public threat-intelligence records) is checked only for
    private keys and deny-list terms, because domains, addresses and e-mails are its content.
  - A line containing the marker "public-ok" is checked only for private keys and deny-list
    terms. Use it for test values, and say why on the same line.

The private deny list (user names, internal domains) never enters the repository: it is
read from the environment variable PUBLIC_DENYLIST (one term per line; a repository secret
in CI) and from the file ~/.config/dactie/denylist. Findings are printed masked, because
CI logs of a public repository are public too.
"""

import gzip
import ipaddress
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ALLOWED_DOMAINS_FILE = ROOT / ".github" / "allowed-domains.txt"
MARKER = "public-ok"

# Top-level domains checked, in lower case only, so that code such as `flag.Int` does not
# count. Kept to those that cannot be confused with file extensions or attribute names
# (.info, .home and .local are left out for that reason).
TLDS = (
    "com|org|net|edu|gov|mil|int|io|de|eu|uk|us|ch|at|nl|fr|it|es|ca|au|jp|cn|ai|dev"
    "|cloud|lan|internal|corp|intranet|arpa"
)
DOMAIN_RE = re.compile(
    rf"(?<![\w.@-])((?:[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?\.)+(?:{TLDS}))(?![\w($-]|\.\w)"
)
EMAIL_RE = re.compile(rf"(?<![\w.+-])([\w.+-]+)@((?:[\w-]+\.)+(?:{TLDS}))(?![\w-]|\.\w)")
IPV4_RE = re.compile(r"(?<![\w.])((?:\d{1,3}\.){3}\d{1,3})(?![\w]|\.\d)")
IPV6_RE = re.compile(r"(?<![\w:.])([0-9a-f]{0,4}(?::[0-9a-f]{0,4}){2,7})(?![\w:])", re.IGNORECASE)
HOME_RE = re.compile(r"(?:/home/|/Users/|[A-Za-z]:\\+Users\\+)([A-Za-z0-9._-]+)")
SSH_USER_RE = re.compile(r"(?<![\w.-])([a-z_][\w.-]*)@(?:minerva\d*|labrada|localhost)\b", re.IGNORECASE)
PRIVATE_KEY_RE = re.compile(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----")
SECRET_WORDS = r"password|passwd|passphrase|secret|token|api[_-]?key|private[_-]?key"
PASSWORD_RES = (
    # A quoted literal: password = "...", "token": "..."
    re.compile(
        rf"""(?:{SECRET_WORDS})\w*["']?\s*[:=]\s*["']([^"'\s$`{{}}<>]{{4,}})["']""", re.IGNORECASE
    ),
    # An environment assignment: POSTGRES_PASSWORD=..., PGPASSWORD=...
    re.compile(rf"""\b[A-Z_]*(?:{SECRET_WORDS.upper()})[A-Z_]*=([^\s"'$`{{}}<>;&|]{{4,}})"""),
    # A config line: password: ..., secret = ...
    re.compile(rf"""^\s*(?:{SECRET_WORDS})\s*[:=]\s*([^\s"'$`{{}}<>#]{{4,}})\s*$""", re.IGNORECASE),
)
PLACEHOLDER_PASSWORDS = {"change-me", "changeme", "example", "password", "secret", "xxxxxx"}
HOME_PLACEHOLDERS = {"user", "username", "runner", "ubuntu", "example"}
ALLOWED_EMAILS = {"noreply@anthropic.com", "noreply@github.com"}
EXAMPLE_EMAIL_DOMAINS = ("example.com", "example.org", "example.net", "users.noreply.github.com")

# Files that must never be tracked: key material, node state, environment files, keystores.
FORBIDDEN_PATHS = re.compile(
    r"(^|/)(key_material[^/]*|storage\.json|slots\.json[^/]*|\.env|[^/]*\.env\.local"
    r"|id_(rsa|ed25519|ecdsa)[^/]*|[^/]*\.(pem|key|p12|pfx|jks)|priv_sk|keystore/[^/]+"
    r"|IssuerSecretKey|SignerConfig)$"
)
# Host files (hosts/*.env) hold host names and private IPs, which rule 1 allows.
FORBIDDEN_PATH_EXCEPTIONS = re.compile(r"(^|/)hosts/[^/]+\.env$")
# Lock files list public package hosts only; scanning their domains adds nothing.
SKIP_DOMAINS_IN = re.compile(r"(^|/)(Cargo\.lock|go\.sum)$")
THIRD_PARTY_DATA = re.compile(r"^workloads/")


def load_allowed_domains():
    allowed = set()
    if ALLOWED_DOMAINS_FILE.exists():
        for line in ALLOWED_DOMAINS_FILE.read_text().splitlines():
            line = line.split("#", 1)[0].strip().lower()
            if line:
                allowed.add(line)
    return allowed


def load_denylist():
    terms = os.environ.get("PUBLIC_DENYLIST", "").splitlines()
    private = Path.home() / ".config" / "dactie" / "denylist"
    if private.exists():
        terms += private.read_text().splitlines()
    terms = {t.strip() for t in terms if t.strip() and not t.strip().startswith("#")}
    return [re.compile(rf"(?<![A-Za-z0-9]){re.escape(t)}(?![A-Za-z0-9])", re.IGNORECASE) for t in terms]


ALLOWED = load_allowed_domains()
DENYLIST = load_denylist()


def under(domain, parents):
    domain = domain.lower()
    return any(domain == p or domain.endswith("." + p) for p in parents)


def mask(value):
    """Shows enough to find the spot, not enough to repeat the value in a public log."""
    if len(value) <= 4:
        return "*" * len(value)
    return value[:2] + "*" * (len(value) - 4) + value[-2:]


def check_line(line, full=True, check_domains=True):
    """Yields (kind, masked value) for every violation in one line of text."""
    for m in PRIVATE_KEY_RE.finditer(line):
        yield "private key", m.group(0)[:16] + "..."
    for pattern in DENYLIST:
        for m in pattern.finditer(line):
            yield "term from the private deny list", mask(m.group(0))
    if not full or MARKER in line:
        return
    for pattern in PASSWORD_RES:
        for m in pattern.finditer(line):
            value = m.group(1)
            # Placeholders such as "..." or "***" hold no letters or digits.
            if value.lower() not in PLACEHOLDER_PASSWORDS and re.search(r"[A-Za-z0-9]", value):
                yield "password or token", mask(value)
    for m in HOME_RE.finditer(line):
        if m.group(1).lower() not in HOME_PLACEHOLDERS:
            yield "home directory (user name)", mask(m.group(1))
    for m in SSH_USER_RE.finditer(line):
        yield "user name in user@host", mask(m.group(1))
    for m in EMAIL_RE.finditer(line):
        address = m.group(0).lower()
        if address not in ALLOWED_EMAILS and not under(m.group(2), EXAMPLE_EMAIL_DOMAINS):
            yield "e-mail address", mask(address)
    for m in IPV4_RE.finditer(line):
        try:
            ip = ipaddress.IPv4Address(m.group(1))
        except ValueError:
            continue
        if ip.is_global:
            yield "public IP address", mask(m.group(1))
    for m in IPV6_RE.finditer(line):
        groups = [g for g in m.group(1).split(":") if g]
        # Rust paths such as aead::{...} look like addresses; real ones have digits.
        if len(groups) < 2 or not re.search(r"\d", m.group(1)):
            continue
        try:
            ip = ipaddress.IPv6Address(m.group(1))
        except ValueError:
            continue
        if ip.is_global:
            yield "public IP address", mask(m.group(1))
    if check_domains:
        for m in DOMAIN_RE.finditer(EMAIL_RE.sub(" ", line)):
            labels = m.group(1).split(".")
            # A one-letter name before the top-level domain is a field access such as o.at.
            if len(labels) == 2 and len(labels[0]) < 2:
                continue
            if not under(m.group(1), ALLOWED):
                yield "domain name", mask(m.group(1))


def file_flags(name):
    """(full check, domain check) for a tracked file."""
    return not THIRD_PARTY_DATA.search(name), not SKIP_DOMAINS_IN.search(name)


def check_text(name, text, full=True, check_domains=True):
    findings = []
    for number, line in enumerate(text.splitlines(), 1):
        for kind, value in check_line(line, full, check_domains):
            findings.append(f"{name}:{number}: {kind}: {value}")
    return findings


def read_text(path):
    data = path.read_bytes()
    if path.suffix == ".gz":
        try:
            data = gzip.decompress(data)
        except OSError:
            return None
    if b"\0" in data[:8192]:
        return None
    return data.decode("utf-8", errors="replace")


def check_path_name(name):
    if FORBIDDEN_PATHS.search(name) and not FORBIDDEN_PATH_EXCEPTIONS.search(name):
        return [f"{name}: key, keystore or env file must not be tracked"]
    return []


def git(*args):
    return subprocess.run(
        ["git", *args], cwd=ROOT, check=True, capture_output=True, text=True, errors="replace"
    ).stdout


def check_tree():
    findings = []
    for name in filter(None, git("ls-files", "-z").split("\0")):
        findings += check_path_name(name)
        path = ROOT / name
        if not path.is_file():
            continue
        text = read_text(path)
        if text is not None:
            findings += check_text(name, text, *file_flags(name))
    return findings


def check_commits(revs):
    findings = []
    for commit in git("rev-list", "--no-merges", *revs).split():
        short = commit[:9]
        findings += check_text(f"commit {short} message", git("log", "-1", "--format=%B", commit))
        # Added lines of every commit, so that something added and removed again within a
        # pull request is caught as well: it stays in the public history.
        current, number = "?", 0
        patch = git("show", "--format=", "--unified=0", "--no-renames", "--text", commit)
        for line in patch.splitlines():
            if line.startswith("+++ "):
                current = line[6:] if line.startswith("+++ b/") else line[4:]
                if current != "/dev/null":
                    findings += check_path_name(current)
            elif line.startswith("@@"):
                hunk = re.search(r"\+(\d+)", line)
                number = int(hunk.group(1)) if hunk else 0
            elif line.startswith("+"):
                for kind, value in check_line(line[1:], *file_flags(current)):
                    findings.append(f"commit {short} {current}:{number}: {kind}: {value}")
                number += 1
    return findings


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in {"tree", "commits", "text"}:
        sys.exit(__doc__)
    mode = sys.argv[1]
    if mode == "tree":
        findings = check_tree()
    elif mode == "commits":
        findings = check_commits(sys.argv[2:])
    else:
        findings = check_text(sys.argv[2] if len(sys.argv) > 2 else "stdin", sys.stdin.read())
    for finding in findings:
        print(finding)
    if findings:
        print(
            f"\n{len(findings)} violation(s) of rule 1 in CONTRIBUTING.md. A public domain that"
            " belongs in the code goes into .github/allowed-domains.txt.",
            file=sys.stderr,
        )
        sys.exit(1)
    print(f"check_public {mode}: no violations")


if __name__ == "__main__":
    main()
