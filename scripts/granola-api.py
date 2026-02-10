#!/usr/bin/env -S uv run
# /// script
# dependencies = ["cryptography", "requests"]
# requires-python = ">=3.11"
# ///
"""
Granola API explorer with encrypted token storage.

The API token is encrypted with a user-provided password (Fernet + PBKDF2)
and stored in ~/.config/grans/api-token.enc. Each invocation prompts for
the password via a zenity GUI dialog — completely outside Claude Code's
reach, with no "remember" checkbox or session caching.

Setup (one-time):
    uv run scripts/granola-api.py --setup

Query:
    uv run scripts/granola-api.py v2/get-documents
    uv run scripts/granola-api.py v1/get-document-panels --body '{"document_id": "abc"}'
    uv run scripts/granola-api.py v2/get-documents | jq '.docs | length'
"""

import argparse
import base64
import getpass
import json
import os
import subprocess
import sys

import requests
from cryptography.fernet import Fernet, InvalidToken
from cryptography.hazmat.primitives.hashes import SHA256
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC

TOKEN_PATH = os.path.expanduser("~/.config/grans/api-token.enc")
API_BASE = "https://api.granola.ai"
SALT_SIZE = 16
KDF_ITERATIONS = 600_000


def _derive_key(password: str, salt: bytes) -> bytes:
    """Derive a Fernet key from a password and salt via PBKDF2."""
    kdf = PBKDF2HMAC(
        algorithm=SHA256(),
        length=32,
        salt=salt,
        iterations=KDF_ITERATIONS,
    )
    return base64.urlsafe_b64encode(kdf.derive(password.encode()))


def _zenity_password(title: str) -> str:
    """Prompt for a password via zenity GUI dialog. Exits on cancel."""
    result = subprocess.run(
        ["zenity", "--password", "--title", title],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print("Cancelled.", file=sys.stderr)
        sys.exit(1)
    return result.stdout.strip()


def setup():
    """Encrypt and store the API token."""
    if os.path.exists(TOKEN_PATH):
        answer = input(
            f"Token file already exists at {TOKEN_PATH}. Replace? [y/N] "
        ).strip().lower()
        if answer != "y":
            print("Aborted.")
            return

    password = _zenity_password("Set password for Granola API token")
    if not password:
        print("Error: empty password.", file=sys.stderr)
        sys.exit(1)

    confirm = _zenity_password("Confirm password")
    if password != confirm:
        print("Error: passwords do not match.", file=sys.stderr)
        sys.exit(1)

    token = getpass.getpass("Paste your Granola API token: ")
    if not token.strip():
        print("Error: empty token.", file=sys.stderr)
        sys.exit(1)

    salt = os.urandom(SALT_SIZE)
    key = _derive_key(password, salt)
    encrypted = Fernet(key).encrypt(token.strip().encode())

    os.makedirs(os.path.dirname(TOKEN_PATH), exist_ok=True)
    with open(TOKEN_PATH, "wb") as f:
        f.write(salt + encrypted)
    os.chmod(TOKEN_PATH, 0o600)

    print(f"Token encrypted and saved to {TOKEN_PATH}")


def read_token() -> str:
    """Prompt for password via zenity, decrypt and return the token."""
    if not os.path.exists(TOKEN_PATH):
        print(
            f"Error: no token file at {TOKEN_PATH}. Run --setup first.",
            file=sys.stderr,
        )
        sys.exit(1)

    with open(TOKEN_PATH, "rb") as f:
        data = f.read()

    salt = data[:SALT_SIZE]
    encrypted = data[SALT_SIZE:]

    password = _zenity_password("Unlock Granola API token")
    key = _derive_key(password, salt)

    try:
        return Fernet(key).decrypt(encrypted).decode()
    except InvalidToken:
        print("Error: wrong password.", file=sys.stderr)
        sys.exit(1)


def query_api(token: str, endpoint: str, body: dict) -> str:
    """Make a POST request to the Granola API and return the response body."""
    url = f"{API_BASE}/{endpoint}"
    resp = requests.post(
        url,
        json=body,
        headers={
            "Authorization": f"Bearer {token}",
            "X-Client-Version": "6.518.0",
        },
    )
    if not resp.ok:
        print(f"HTTP {resp.status_code}: {resp.reason}", file=sys.stderr)
        print(resp.text, file=sys.stderr)
        sys.exit(1)
    return resp.text


def main():
    parser = argparse.ArgumentParser(
        description="Query the Granola API with encrypted token storage.",
        epilog="Examples:\n"
        "  uv run scripts/granola-api.py --setup\n"
        "  uv run scripts/granola-api.py v2/get-documents\n"
        '  uv run scripts/granola-api.py v1/get-document-panels --body \'{"document_id": "abc"}\'\n',
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--setup",
        action="store_true",
        help="Encrypt and store your API token (one-time setup).",
    )
    parser.add_argument(
        "endpoint",
        nargs="?",
        help="API endpoint including version, e.g. v2/get-documents",
    )
    parser.add_argument(
        "--body",
        default="{}",
        help='JSON request body (default: "{}")',
    )
    args = parser.parse_args()

    if not args.setup and not args.endpoint:
        parser.error("either --setup or an endpoint is required")

    if args.setup:
        setup()
        return

    try:
        body = json.loads(args.body)
    except json.JSONDecodeError as e:
        print(f"Error: invalid JSON in --body: {e}", file=sys.stderr)
        sys.exit(1)

    token = read_token()
    result = query_api(token, args.endpoint, body)
    print(result)


if __name__ == "__main__":
    main()
