# /// script
# requires-python = ">=3.10"
# dependencies = ["cryptography"]
# ///
"""Prototype: decrypt Granola's encrypted local token store on Windows.

Granola (an Electron app) now stores its Supabase auth token in an encrypted
`supabase.json.enc` file instead of the old plaintext `supabase.json`. The
encryption is the standard Chromium/Electron `os_crypt` + safeStorage scheme:

    Local State -> os_crypt.encrypted_key  (DPAPI-wrapped AES-256 master key)
    storage.dek                            (master-key-encrypted data key)
    supabase.json.enc                      (data-key-encrypted token JSON)

DPAPI ties the master key to the current Windows user, so any process running
as that user (including this script and, eventually, grans) can decrypt it.

Usage:
    uv run scripts/granola-token.py           # verbose: report format + expiry
    uv run scripts/granola-token.py --raw     # print only the token on stdout
                                              #   e.g. grans sync --token "$(...)"

Diagnostics always go to stderr, so --raw stdout is exactly the token.
"""

from __future__ import annotations

import base64
import ctypes
import ctypes.wintypes as wt
import datetime as dt
import json
import os
import sys
from pathlib import Path

from cryptography.hazmat.primitives.ciphers.aead import AESGCM


def log(*args, **kwargs) -> None:
    kwargs.setdefault("file", sys.stderr)
    print(*args, **kwargs)


def granola_dir() -> Path:
    appdata = os.environ.get("APPDATA")
    if not appdata:
        sys.exit("APPDATA not set; this prototype is Windows-only.")
    return Path(appdata) / "Granola"


# --- DPAPI (CryptUnprotectData) via ctypes, no pywin32 dependency ----------


class DATA_BLOB(ctypes.Structure):
    _fields_ = [("cbData", wt.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]


def dpapi_decrypt(blob: bytes) -> bytes:
    src = DATA_BLOB(len(blob), ctypes.cast(ctypes.c_char_p(blob), ctypes.POINTER(ctypes.c_char)))
    out = DATA_BLOB()
    ok = ctypes.windll.crypt32.CryptUnprotectData(
        ctypes.byref(src), None, None, None, None, 0, ctypes.byref(out)
    )
    if not ok:
        raise OSError(f"CryptUnprotectData failed (GetLastError={ctypes.get_last_error()})")
    try:
        return ctypes.string_at(out.pbData, out.cbData)
    finally:
        ctypes.windll.kernel32.LocalFree(out.pbData)


# --- os_crypt key + AES-GCM helpers ----------------------------------------


def master_key(g: Path) -> bytes:
    local_state = json.loads((g / "Local State").read_text(encoding="utf-8"))
    encrypted_key = base64.b64decode(local_state["os_crypt"]["encrypted_key"])
    if encrypted_key[:5] != b"DPAPI":
        raise ValueError(f"unexpected key prefix: {encrypted_key[:5]!r}")
    key = dpapi_decrypt(encrypted_key[5:])
    log(f"[master key] {len(key)} bytes")
    return key


def try_gcm(label: str, key: bytes, blob: bytes) -> bytes | None:
    """Try several framings of an os_crypt-style GCM blob; return plaintext of
    the first that authenticates, else None. The GCM auth tag guarantees a wrong
    framing fails rather than returning garbage."""
    hyps = [
        ("strip 'v10', nonce=12", 3, 12),
        ("strip 'v10;', nonce=12", 4, 12),
        ("no prefix, nonce=12", 0, 12),
    ]
    aes = AESGCM(key)
    for name, prefix_len, nonce_len in hyps:
        body = blob[prefix_len:]
        nonce, ct_tag = body[:nonce_len], body[nonce_len:]
        try:
            pt = aes.decrypt(nonce, ct_tag, None)
        except Exception:
            continue
        log(f"[{label}] framing OK: {name} -> {len(pt)} bytes plaintext")
        return pt
    log(f"[{label}] no framing authenticated (prefix={blob[:4]!r}, len={len(blob)})")
    return None


def maybe_b64(pt: bytes) -> bytes:
    """The data key is stored as base64 text inside its GCM blob; token JSON is
    not. Decode base64-looking plaintext, else pass through."""
    stripped = pt.strip()
    try:
        decoded = base64.b64decode(stripped, validate=True)
    except Exception:
        return pt
    if base64.b64encode(decoded) == stripped:
        log(f"    (plaintext was base64 -> {len(decoded)} raw bytes)")
        return decoded
    return pt


def extract_token(g: Path) -> str:
    key = master_key(g)

    dek_pt = try_gcm("storage.dek", key, (g / "storage.dek").read_bytes())
    if dek_pt is None:
        sys.exit("Could not decrypt storage.dek; framing hypotheses exhausted.")
    data_key = maybe_b64(dek_pt)
    log(f"[data key] {len(data_key)} bytes")

    token_pt = try_gcm("supabase.json.enc", data_key, (g / "supabase.json.enc").read_bytes())
    if token_pt is None:
        sys.exit("Could not decrypt supabase.json.enc with the data key.")

    token_json = json.loads(token_pt)
    wt_field = token_json.get("workos_tokens")
    if isinstance(wt_field, str):
        wt_field = json.loads(wt_field)
    access_token = (wt_field or {}).get("access_token") if wt_field else None
    if not access_token:
        sys.exit("No access_token found in decrypted payload.")
    return access_token


def jwt_exp(token: str) -> int | None:
    try:
        payload_b64 = token.split(".")[1]
        payload_b64 += "=" * (-len(payload_b64) % 4)
        return json.loads(base64.urlsafe_b64decode(payload_b64)).get("exp")
    except Exception:
        return None


def main() -> None:
    raw = "--raw" in sys.argv[1:]
    g = granola_dir()
    log(f"Granola dir: {g}")

    access_token = extract_token(g)

    if raw:
        sys.stdout.write(access_token)
        return

    exp = jwt_exp(access_token)
    print("\n=== access_token ===")
    print(f"length: {len(access_token)}")
    print(f"prefix: {access_token[:24]}...")
    if exp is not None:
        exp_dt = dt.datetime.fromtimestamp(exp)
        status = "EXPIRED" if dt.datetime.now() > exp_dt else "valid"
        print(f"expires: {exp_dt}  ({status})")


if __name__ == "__main__":
    main()
