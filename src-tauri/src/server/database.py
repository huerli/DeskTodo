"""Executed over SSH by DeskTodo. Receives only client-encrypted bytes."""
import os
import sqlite3
import struct
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

MAX_BYTES = 64 * 1024 * 1024
SCHEMA = """CREATE TABLE IF NOT EXISTS snapshots (
    device_id TEXT PRIMARY KEY,
    encrypted_payload BLOB NOT NULL,
    uploaded_at TEXT NOT NULL,
    format_version INTEGER NOT NULL CHECK(format_version = 1),
    revision INTEGER NOT NULL DEFAULT 1
)"""


def valid_envelope(payload, device_id=None):
    if len(payload) < 48 or len(payload) > MAX_BYTES or payload[:4] != b"DTD1":
        raise ValueError("Unsupported encrypted snapshot format")
    identity = str(uuid.UUID(bytes=payload[4:20]))
    if device_id is not None and identity != device_id:
        raise ValueError("Snapshot device identity mismatch")
    return identity


def prepare_schema(db):
    db.execute(SCHEMA)
    columns = {row[1] for row in db.execute("PRAGMA table_info(snapshots)")}
    if "revision" not in columns:
        db.execute("ALTER TABLE snapshots ADD COLUMN revision INTEGER NOT NULL DEFAULT 1")


def main():
    if sqlite3.sqlite_version_info < (3, 24, 0):
        raise ValueError("SQLite 3.24 or newer is required")
    mode, filename = sys.argv[1:3]
    if mode == "check":
        if Path(filename).exists():
            if not Path(filename).is_file() or not os.access(filename, os.W_OK):
                raise ValueError("Database file is not writable")
            with sqlite3.connect(Path(filename).absolute().as_uri() + "?mode=ro", uri=True) as db:
                if db.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                    raise ValueError("Database integrity check failed")
        return

    if mode == "fetch":
        group = str(uuid.UUID(sys.argv[3]))
        if not Path(filename).is_file():
            sys.stdout.buffer.write(struct.pack("<Q", 0))
            return
        with sqlite3.connect(Path(filename).absolute().as_uri() + "?mode=ro", uri=True, timeout=10) as db:
            tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if "snapshots" not in tables:
                row = None
            else:
                columns = {row[1] for row in db.execute("PRAGMA table_info(snapshots)")}
                revision = "revision" if "revision" in columns else "1"
                row = db.execute("SELECT encrypted_payload, " + revision + " FROM snapshots WHERE device_id=?", (group,)).fetchone()
        if row is None:
            sys.stdout.buffer.write(struct.pack("<Q", 0))
        else:
            payload, revision = row
            valid_envelope(payload, group)
            sys.stdout.buffer.write(struct.pack("<Q", revision) + payload)
        return

    if mode == "cas":
        request = sys.stdin.buffer.read(MAX_BYTES + 9)
        if len(request) < 8:
            raise ValueError("Invalid sync revision")
        expected = struct.unpack("<Q", request[:8])[0]
        payload = request[8:]
        group = valid_envelope(payload)
        with sqlite3.connect(filename, timeout=10) as db:
            db.execute("PRAGMA synchronous=FULL")
            db.execute("BEGIN IMMEDIATE")
            prepare_schema(db)
            row = db.execute("SELECT revision FROM snapshots WHERE device_id=?", (group,)).fetchone()
            current = row[0] if row else 0
            if current != expected:
                sys.stdout.buffer.write(b"\x00")
                return
            if current >= 2**63-1:
                raise ValueError("Sync revision exhausted")
            db.execute("""INSERT INTO snapshots(device_id, encrypted_payload, uploaded_at, format_version, revision)
                VALUES (?, ?, ?, 1, ?)
                ON CONFLICT(device_id) DO UPDATE SET encrypted_payload=excluded.encrypted_payload,
                    uploaded_at=excluded.uploaded_at, format_version=1, revision=excluded.revision""",
                (group, sqlite3.Binary(payload), datetime.now(timezone.utc).isoformat(), current + 1))
        sys.stdout.buffer.write(b"\x01")
        return

    if mode == "upload":
        payload = sys.stdin.buffer.read(MAX_BYTES + 1)
        device = valid_envelope(payload)
        with sqlite3.connect(filename, timeout=10) as db:
            db.execute("PRAGMA synchronous=FULL")
            db.execute("BEGIN IMMEDIATE")
            prepare_schema(db)
            db.execute("""INSERT INTO snapshots(device_id, encrypted_payload, uploaded_at, format_version)
                VALUES (?, ?, ?, 1)
                ON CONFLICT(device_id) DO UPDATE SET
                    encrypted_payload=excluded.encrypted_payload,
                    uploaded_at=excluded.uploaded_at,
                    format_version=excluded.format_version, revision=snapshots.revision+1""",
                (device, sqlite3.Binary(payload), datetime.now(timezone.utc).isoformat()))
        return

    if mode == "download":
        device = str(uuid.UUID(sys.argv[3]))
        if not Path(filename).is_file():
            raise ValueError("Server database does not exist; upload a snapshot first")
        with sqlite3.connect(Path(filename).absolute().as_uri() + "?mode=ro", uri=True, timeout=10) as db:
            row = db.execute("SELECT encrypted_payload, format_version FROM snapshots WHERE device_id=?", (device,)).fetchone()
        if row is None:
            raise ValueError("No encrypted snapshot for this device")
        payload, version = row
        if version != 1:
            raise ValueError("Unsupported encrypted snapshot version")
        valid_envelope(payload, device)
        sys.stdout.buffer.write(payload)
        return
    raise ValueError("Unsupported database operation")


if __name__ == "__main__":
    os.umask(0o077)
    try:
        main()
    except Exception as error:
        print("DeskTodo database operation failed: " + str(error), file=sys.stderr)
        sys.exit(1)
