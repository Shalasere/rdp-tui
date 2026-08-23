"""Encrypted local password storage, separate from profile data."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

from cryptography.fernet import Fernet, InvalidToken

CONFIG_DIR = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "rdp-tui"
KEY_PATH = CONFIG_DIR / ".password-key"
SECRETS_PATH = CONFIG_DIR / "secrets.json"


class SecretStoreError(RuntimeError):
    """The encrypted password store could not be read or written."""


def _atomic_write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w", encoding="utf-8") as file:
        file.write(content)
    os.chmod(temporary, 0o600)
    temporary.replace(path)


def _key(key_path: Path, create: bool = False) -> bytes | None:
    try:
        return key_path.read_bytes()
    except FileNotFoundError:
        if not create:
            return None
    key = Fernet.generate_key()
    _atomic_write(key_path, key.decode("ascii"))
    return key


def _load(secrets_path: Path) -> dict[str, str]:
    try:
        with secrets_path.open(encoding="utf-8") as file:
            data = json.load(file)
    except FileNotFoundError:
        return {}
    except (OSError, json.JSONDecodeError) as exc:
        raise SecretStoreError(f"Could not read encrypted password store: {exc}") from exc
    if not isinstance(data, dict) or not all(
        isinstance(key, str) and isinstance(value, str) for key, value in data.items()
    ):
        raise SecretStoreError("Encrypted password store has an invalid format.")
    return data


def _file_password(profile_id: str, secrets_path: Path = SECRETS_PATH, key_path: Path = KEY_PATH) -> str | None:
    encrypted = _load(secrets_path).get(profile_id)
    if encrypted is None:
        return None
    key = _key(key_path)
    if key is None:
        raise SecretStoreError("Password key is missing; saved passwords cannot be decrypted.")
    try:
        return Fernet(key).decrypt(encrypted.encode("ascii")).decode("utf-8")
    except (InvalidToken, UnicodeDecodeError, ValueError) as exc:
        raise SecretStoreError("Saved password could not be decrypted.") from exc


def _save_file_password(
    profile_id: str, password: str, secrets_path: Path = SECRETS_PATH, key_path: Path = KEY_PATH
) -> None:
    try:
        key = _key(key_path, create=True)
        assert key is not None
        data = _load(secrets_path)
        data[profile_id] = Fernet(key).encrypt(password.encode("utf-8")).decode("ascii")
        _atomic_write(secrets_path, json.dumps(data, indent=2) + "\n")
    except (OSError, ValueError) as exc:
        raise SecretStoreError(f"Could not save encrypted password: {exc}") from exc


def _delete_file_password(profile_id: str, secrets_path: Path = SECRETS_PATH) -> None:
    data = _load(secrets_path)
    if profile_id in data:
        del data[profile_id]
        _atomic_write(secrets_path, json.dumps(data, indent=2) + "\n")


def _keyring_command(action: str, profile_id: str) -> list[str]:
    return ["secret-tool", action, "application", "rdp-tui", "profile", profile_id]


def keyring_available() -> bool:
    """Check for a running Secret Service without activating a wallet."""
    if not shutil.which("secret-tool") or not shutil.which("busctl"):
        return False
    result = subprocess.run(
        ["busctl", "--user", "--no-pager", "--no-legend", "list"], text=True, capture_output=True, check=False
    )
    for line in result.stdout.splitlines():
        columns = line.split()
        if len(columns) >= 2 and columns[0] == "org.freedesktop.secrets":
            return columns[1] != "-"
    return False


def resolved_backend(backend: str) -> str:
    """Apply Remmina-style automatic selection without starting a keyring."""
    if backend == "automatic":
        return "keyring" if keyring_available() else "encrypted_file"
    if backend in {"encrypted_file", "keyring"}:
        return backend
    raise SecretStoreError(f"Unknown password backend: {backend}")


def _keyring_password(profile_id: str) -> str | None:
    if not shutil.which("secret-tool"):
        raise SecretStoreError("secret-tool is not installed for the Keyring backend.")
    result = subprocess.run(_keyring_command("lookup", profile_id), text=True, capture_output=True, check=False)
    return result.stdout.removesuffix("\n") if result.returncode == 0 else None


def _save_keyring_password(profile_id: str, password: str) -> None:
    if not shutil.which("secret-tool"):
        raise SecretStoreError("secret-tool is not installed for the Keyring backend.")
    result = subprocess.run(
        ["secret-tool", "store", "--label=rdp-tui password", "application", "rdp-tui", "profile", profile_id],
        input=password,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise SecretStoreError(result.stderr.strip() or "Secret Service rejected the password.")


def _delete_keyring_password(profile_id: str) -> None:
    if shutil.which("secret-tool"):
        subprocess.run(_keyring_command("clear", profile_id), text=True, capture_output=True, check=False)


def password_for(profile_id: str, backend: str = "automatic") -> str | None:
    """Get a saved password from the selected storage backend."""
    backend = resolved_backend(backend)
    if backend == "encrypted_file":
        return _file_password(profile_id)
    if backend == "keyring":
        return _keyring_password(profile_id)
    raise SecretStoreError(f"Unknown password backend: {backend}")


def save_password(profile_id: str, password: str, backend: str = "automatic") -> None:
    """Save a password in the selected storage backend."""
    backend = resolved_backend(backend)
    if backend == "encrypted_file":
        _save_file_password(profile_id, password)
    elif backend == "keyring":
        _save_keyring_password(profile_id, password)
    else:
        raise SecretStoreError(f"Unknown password backend: {backend}")


def delete_password(profile_id: str, backend: str = "automatic") -> None:
    """Delete a password from the selected storage backend."""
    backend = resolved_backend(backend)
    if backend == "encrypted_file":
        _delete_file_password(profile_id)
    elif backend == "keyring":
        _delete_keyring_password(profile_id)
    else:
        raise SecretStoreError(f"Unknown password backend: {backend}")
