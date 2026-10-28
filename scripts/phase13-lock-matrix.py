#!/usr/bin/env python3
"""Phase 13 ground truth: PostgreSQL row locks and foreign keys under concurrency.

The script starts a DISPOSABLE PostgreSQL 17 cluster. The cluster listens only
on a unix socket, under a short `/tmp/plm.*` directory (the macOS socket path
limit), and is stopped and deleted at the end. The script never connects
anywhere else. It drives two concurrent sessions (A, B) through explicit step
schedules over the PostgreSQL wire protocol. A third session, the observer,
detects lock waits through `pg_locks`. Every cell is recorded.

Blocking is detected WITHOUT sleeps used as synchronization. After a statement
is sent, the script polls (bounded) until either the statement's response
arrives or the observer sees the backend with an ungranted lock in `pg_locks`.
A step that does neither within the bound is recorded as INCONCLUSIVE, never as
a result.

Step semantics: steps run in the listed global order. A step whose session is
still blocked is queued and runs as soon as that session's statement
completes. This is what a client does: it issues its next statement when the
previous one returns. A step can carry `when`, a condition on the first value
returned by an earlier labeled step; when the condition is false the step is
skipped. At the end, every blocked session is drained, so a deadlock is
resolved by the server's detector, with a bound.

Usage:
  python3 scripts/phase13-lock-matrix.py                 # generate, run, write outputs
  python3 scripts/phase13-lock-matrix.py --only main/    # cells whose id starts with a prefix
  python3 scripts/phase13-lock-matrix.py --from-cells fixtures/phase13/lock-matrix/cells.json
  python3 scripts/phase13-lock-matrix.py --write-cells-only

Python 3 standard library only. Server binaries default to
/opt/homebrew/opt/postgresql@17/bin (override: --pg-bin or PHASE13_PG_BIN).
"""

from __future__ import annotations

import argparse
import atexit
import collections
import datetime
import json
import os
import select
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_PG_BIN = os.environ.get("PHASE13_PG_BIN", "/opt/homebrew/opt/postgresql@17/bin")
CELLS_PATH = REPO / "fixtures/phase13/lock-matrix/cells.json"
RESULTS_PATH = REPO / "reports/phase13/lock-matrix/results.json"
SUMMARY_PATH = REPO / "reports/phase13/lock-matrix/summary.md"

SETTLE_TIMEOUT = 10.0  # seconds: a statement must complete or be seen waiting
DRAIN_TIMEOUT = 15.0  # seconds: blocked sessions must finish after the schedule
DEADLOCK_TIMEOUT = "100ms"  # detection latency only; does not change semantics
PORT = 5432
DATABASE = "plm"

# --------------------------------------------------------------------------
# Disposable cluster
# --------------------------------------------------------------------------


def clean_env() -> dict:
    env = {k: v for k, v in os.environ.items() if not k.startswith("PG")}
    env["LC_ALL"] = "C"
    return env


class Cluster:
    def __init__(self, pg_bin: str):
        self.bin = Path(pg_bin)
        for tool in ("initdb", "pg_ctl", "postgres", "psql"):
            if not (self.bin / tool).exists():
                sys.exit(f"missing {self.bin / tool}")
        self.dir = Path(tempfile.mkdtemp(prefix="plm.", dir="/tmp"))
        self.data = self.dir / "d"
        self.log = self.dir / "server.log"
        self.socket_path = str(self.dir / f".s.PGSQL.{PORT}")
        if len(self.socket_path) > 100:
            sys.exit(f"socket path too long: {self.socket_path}")
        self.started = False

    def tool(self, name: str) -> str:
        return str(self.bin / name)

    def start(self) -> None:
        subprocess.run(
            [self.tool("initdb"), "-D", str(self.data), "-U", "postgres", "-A", "trust", "-E", "UTF8",
             "--locale=C", "--no-sync", "--no-instructions"],
            check=True, env=clean_env(), stdout=subprocess.DEVNULL)
        with open(self.data / "postgresql.conf", "a") as conf:
            conf.write(
                "\n# phase13-lock-matrix: disposable, unix socket only\n"
                "listen_addresses = ''\n"
                f"unix_socket_directories = '{self.dir}'\n"
                f"port = {PORT}\n"
                "fsync = off\nsynchronous_commit = off\nfull_page_writes = off\n"
                f"deadlock_timeout = '{DEADLOCK_TIMEOUT}'\n"
                "max_connections = 40\n"
                "default_transaction_isolation = 'read committed'\n")
        subprocess.run(
            [self.tool("pg_ctl"), "-D", str(self.data), "-l", str(self.log), "-w", "-t", "60", "start"],
            check=True, env=clean_env(), stdout=subprocess.DEVNULL)
        self.started = True
        self.psql("postgres", f"CREATE DATABASE {DATABASE}")

    def psql(self, database: str, sql: str) -> str:
        out = subprocess.run(
            [self.tool("psql"), "-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-h", str(self.dir),
             "-p", str(PORT), "-U", "postgres", "-d", database, "-c", sql],
            check=True, env=clean_env(), capture_output=True, text=True)
        return out.stdout.strip()

    def stop(self) -> None:
        if self.started:
            subprocess.run([self.tool("pg_ctl"), "-D", str(self.data), "-m", "immediate", "-w", "stop"],
                           env=clean_env(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.started = False
        shutil.rmtree(self.dir, ignore_errors=True)


# --------------------------------------------------------------------------
# Minimal PostgreSQL wire-protocol client (simple query protocol, trust auth)
# --------------------------------------------------------------------------


class PgError(Exception):
    pass


class Result:
    def __init__(self, sql):
        self.sql = sql
        self.columns = []
        self.rows = []
        self.tag = None
        self.error = None
        self.notices = []
        self.txstatus = None

    def first(self):
        return self.rows[0][0] if self.rows and self.rows[0] else None

    def to_json(self, max_rows=20):
        out = {"tag": self.tag, "txstatus": self.txstatus}
        if self.rows:
            out["columns"] = self.columns
            out["rows"] = self.rows[:max_rows]
        if self.error:
            out["error"] = self.error
        if self.notices:
            out["notices"] = [n.get("message") for n in self.notices]
        return out


ERROR_FIELDS = {"C": "sqlstate", "M": "message", "D": "detail", "H": "hint", "n": "constraint",
                "t": "table", "R": "routine", "V": "severity"}


def parse_fields(body: bytes) -> dict:
    out, off = {}, 0
    while off < len(body) and body[off] != 0:
        code = chr(body[off])
        end = body.index(b"\0", off + 1)
        if code in ERROR_FIELDS:
            out[ERROR_FIELDS[code]] = body[off + 1:end].decode(errors="replace")
        off = end + 1
    return out


class Conn:
    def __init__(self, socket_path: str, app: str):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(socket_path)
        self.buf = bytearray()
        self.pid = None
        self.params = {}
        payload = b"".join(k.encode() + b"\0" + v.encode() + b"\0" for k, v in (
            ("user", "postgres"), ("database", DATABASE), ("application_name", app),
            ("client_encoding", "UTF8"))) + b"\0"
        self.sock.sendall(struct.pack("!ii", len(payload) + 8, 196608) + payload)
        self.inflight = Result(None)
        r = self.poll(10)
        if r is None or r.error:
            raise PgError(f"startup failed: {r.error if r else 'timeout'}")

    def send(self, sql: str) -> None:
        if self.inflight is not None:
            raise PgError("statement already in flight")
        q = sql.encode() + b"\0"
        self.sock.sendall(b"Q" + struct.pack("!i", len(q) + 4) + q)
        self.inflight = Result(sql)

    def busy(self) -> bool:
        return self.inflight is not None

    def poll(self, timeout: float):
        """Read for up to `timeout` seconds; the finished Result, or None."""
        end = time.monotonic() + timeout
        while True:
            r = self._messages()
            if r is not None:
                return r
            remaining = end - time.monotonic()
            if remaining < 0:
                return None
            ready, _, _ = select.select([self.sock], [], [], remaining)
            if not ready:
                return None
            data = self.sock.recv(1 << 16)
            if not data:
                raise PgError("connection closed by server")
            self.buf += data

    def _messages(self):
        while len(self.buf) >= 5:
            kind = chr(self.buf[0])
            length = struct.unpack_from("!i", self.buf, 1)[0]
            if len(self.buf) < 1 + length:
                return None
            body = bytes(self.buf[5:1 + length])
            del self.buf[:1 + length]
            r = self.inflight
            if kind == "R":
                if struct.unpack_from("!i", body, 0)[0] != 0:
                    raise PgError("only trust authentication is supported")
            elif kind == "S":
                k, v, _ = body.split(b"\0", 2)
                self.params[k.decode()] = v.decode()
            elif kind == "K":
                self.pid = struct.unpack_from("!i", body, 0)[0]
            elif kind == "T":
                n, off, cols = struct.unpack_from("!h", body, 0)[0], 2, []
                for _ in range(n):
                    end = body.index(b"\0", off)
                    cols.append(body[off:end].decode())
                    off = end + 1 + 18
                r.columns = cols
            elif kind == "D":
                n, off, row = struct.unpack_from("!h", body, 0)[0], 2, []
                for _ in range(n):
                    size = struct.unpack_from("!i", body, off)[0]
                    off += 4
                    if size < 0:
                        row.append(None)
                    else:
                        row.append(body[off:off + size].decode())
                        off += size
                r.rows.append(row)
            elif kind == "C":
                r.tag = body[:-1].decode()
            elif kind == "I":
                r.tag = ""
            elif kind == "E":
                r.error = parse_fields(body)
            elif kind == "N":
                if r is not None:
                    r.notices.append(parse_fields(body))
            elif kind == "Z":
                r.txstatus = chr(body[0])
                self.inflight = None
                return r
        return None

    def query(self, sql: str, timeout: float = 10.0) -> Result:
        self.send(sql)
        r = self.poll(timeout)
        if r is None:
            raise PgError(f"observer query timed out: {sql}")
        return r

    def close(self) -> None:
        try:
            self.sock.sendall(b"X" + struct.pack("!i", 4))
        except OSError:
            pass
        self.sock.close()


# --------------------------------------------------------------------------
# Matrix definition
# --------------------------------------------------------------------------

ISOLATIONS = ["READ COMMITTED", "REPEATABLE READ", "SERIALIZABLE"]
ISO_SHORT = {"READ COMMITTED": "rc", "REPEATABLE READ": "rr", "SERIALIZABLE": "ser"}

VARIANTS = {
    "base": {
        "description": "users(id int primary key, name text); workspaces(id int primary key, owner_id int references users(id), name text)",
        "ddl": [
            "DROP TABLE IF EXISTS folders, workspaces, users CASCADE",
            "CREATE TABLE users (id int PRIMARY KEY, name text)",
            "CREATE TABLE workspaces (id int PRIMARY KEY, owner_id int REFERENCES users(id), name text)",
        ],
        "seed": [
            "INSERT INTO users (id, name) VALUES (1, 'u1'), (2, 'u2')",
            "INSERT INTO workspaces (id, owner_id, name) VALUES (10, 2, 'w10')",
        ],
        "tables": ["users", "workspaces"],
    },
    "email": {
        "description": "base plus users.email text UNIQUE (a key column that no foreign key references)",
        "ddl": [
            "DROP TABLE IF EXISTS folders, workspaces, users CASCADE",
            "CREATE TABLE users (id int PRIMARY KEY, name text, email text UNIQUE)",
            "CREATE TABLE workspaces (id int PRIMARY KEY, owner_id int REFERENCES users(id), name text)",
        ],
        "seed": [
            "INSERT INTO users (id, name, email) VALUES (1, 'u1', 'u1@x'), (2, 'u2', 'u2@x')",
            "INSERT INTO workspaces (id, owner_id, name) VALUES (10, 2, 'w10')",
        ],
        "tables": ["users", "workspaces"],
    },
    "cascade": {
        "description": "base with workspaces.owner_id REFERENCES users(id) ON DELETE CASCADE; user 1 owns workspace 11",
        "ddl": [
            "DROP TABLE IF EXISTS folders, workspaces, users CASCADE",
            "CREATE TABLE users (id int PRIMARY KEY, name text)",
            "CREATE TABLE workspaces (id int PRIMARY KEY, owner_id int REFERENCES users(id) ON DELETE CASCADE, name text)",
        ],
        "seed": [
            "INSERT INTO users (id, name) VALUES (1, 'u1'), (2, 'u2')",
            "INSERT INTO workspaces (id, owner_id, name) VALUES (10, 2, 'w10'), (11, 1, 'w11')",
        ],
        "tables": ["users", "workspaces"],
    },
    "deferred": {
        "description": "base with workspaces.owner_id REFERENCES users(id) DEFERRABLE INITIALLY DEFERRED",
        "ddl": [
            "DROP TABLE IF EXISTS folders, workspaces, users CASCADE",
            "CREATE TABLE users (id int PRIMARY KEY, name text)",
            "CREATE TABLE workspaces (id int PRIMARY KEY, owner_id int REFERENCES users(id) DEFERRABLE INITIALLY DEFERRED, name text)",
        ],
        "seed": [
            "INSERT INTO users (id, name) VALUES (1, 'u1'), (2, 'u2')",
            "INSERT INTO workspaces (id, owner_id, name) VALUES (10, 2, 'w10')",
        ],
        "tables": ["users", "workspaces"],
    },
    "nulls": {
        "description": "users plus folders(id, owner_id references users, parent_id int, name, UNIQUE(owner_id, parent_id, name)) (NULLS DISTINCT, the default)",
        "ddl": [
            "DROP TABLE IF EXISTS folders, workspaces, users CASCADE",
            "CREATE TABLE users (id int PRIMARY KEY, name text)",
            "CREATE TABLE folders (id int PRIMARY KEY, owner_id int REFERENCES users(id), parent_id int, name text, UNIQUE (owner_id, parent_id, name))",
        ],
        "seed": ["INSERT INTO users (id, name) VALUES (1, 'u1'), (2, 'u2')"],
        "tables": ["users", "folders"],
    },
}

# What session A holds on users row 1 (the parent).
A_LOCKS = {
    "none": "SELECT id, name FROM users WHERE id = 1",
    "for_update": "SELECT id FROM users WHERE id = 1 FOR UPDATE",
    "for_no_key_update": "SELECT id FROM users WHERE id = 1 FOR NO KEY UPDATE",
    "for_share": "SELECT id FROM users WHERE id = 1 FOR SHARE",
    "for_key_share": "SELECT id FROM users WHERE id = 1 FOR KEY SHARE",
    "update_nonkey": "UPDATE users SET name = 'a' WHERE id = 1",
    "update_key_id": "UPDATE users SET id = 3 WHERE id = 1",
    "delete": "DELETE FROM users WHERE id = 1",
    # email variant only
    "update_key_email": "UPDATE users SET email = 'u1-new@x' WHERE id = 1",
    "update_key_email_same": "UPDATE users SET email = 'u1@x', name = 'a' WHERE id = 1",
}
A_LOCKS_BASE = ["none", "for_update", "for_no_key_update", "for_share", "for_key_share",
                "update_nonkey", "update_key_id", "delete"]
A_LOCKS_EMAIL = ["update_nonkey", "update_key_email", "update_key_email_same"]

# What session B does to users row 1 (or to a child referencing it).
B_ACTIONS = {
    "insert_child": "INSERT INTO workspaces (id, owner_id, name) VALUES (20, 1, 'w20')",
    "update_child_fk": "UPDATE workspaces SET owner_id = 1 WHERE id = 10",
    "for_key_share": "SELECT id FROM users WHERE id = 1 FOR KEY SHARE",
    "for_share": "SELECT id FROM users WHERE id = 1 FOR SHARE",
    "for_update": "SELECT id FROM users WHERE id = 1 FOR UPDATE",
    "update_nonkey": "UPDATE users SET name = 'b' WHERE id = 1",
    "delete_parent": "DELETE FROM users WHERE id = 1",
    # cascade variant: actions on user 1's existing child row (workspace 11)
    "child_update_name": "UPDATE workspaces SET name = 'b' WHERE id = 11",
    "child_for_key_share": "SELECT id FROM workspaces WHERE id = 11 FOR KEY SHARE",
    "child_delete": "DELETE FROM workspaces WHERE id = 11",
}
B_ACTIONS_MAIN = ["insert_child", "update_child_fk", "for_key_share", "for_share", "for_update",
                  "update_nonkey", "delete_parent"]

# The common description of row-lock modes (PostgreSQL manual, table "Conflicting
# Row-Level Locks"), plus the usual reading: non-key UPDATE = FOR NO KEY UPDATE,
# key UPDATE / DELETE = FOR UPDATE, foreign-key check = FOR KEY SHARE.
COMMON_HELD = {
    "none": None, "for_update": "FU", "for_no_key_update": "FNKU", "for_share": "FS",
    "for_key_share": "FKS", "update_nonkey": "FNKU", "update_key_id": "FU", "delete": "FU",
    "update_key_email": "FU", "update_key_email_same": "FNKU",
}
COMMON_REQUESTED = {
    "insert_child": "FKS", "update_child_fk": "FKS", "for_key_share": "FKS", "for_share": "FS",
    "for_update": "FU", "update_nonkey": "FNKU", "delete_parent": "FU",
}
CONFLICTS = {
    "FKS": {"FU"},
    "FS": {"FNKU", "FU"},
    "FNKU": {"FS", "FNKU", "FU"},
    "FU": {"FKS", "FS", "FNKU", "FU"},
}


def common_blocks(a_lock: str, b_action: str):
    held, req = COMMON_HELD.get(a_lock), COMMON_REQUESTED.get(b_action)
    if req is None:
        return None
    return held is not None and held in CONFLICTS[req]


def step(s, sql, label=None, when=None):
    d = {"s": s, "sql": sql}
    if label:
        d["label"] = label
    if when:
        d["when"] = when
    return d


def end_sql(end):
    return "COMMIT" if end == "commit" else "ROLLBACK"


def cell(cid, family, variant, isolation, params, steps, focus, note=""):
    v = VARIANTS[variant]
    return {
        "id": cid, "family": family, "variant": variant, "isolation": isolation, "params": params,
        "ddl": v["ddl"], "seed": v["seed"], "tables": v["tables"], "steps": steps, "focus": focus,
        "note": note,
    }


def generate_cells():
    cells = []

    # 1. Main matrix: A holds a lock on the parent, B acts, A ends.
    for variant, locks in (("base", A_LOCKS_BASE), ("email", A_LOCKS_EMAIL)):
        for iso in ISOLATIONS:
            for a_lock in locks:
                for b_action in B_ACTIONS_MAIN:
                    for end in ("commit", "rollback"):
                        cid = f"main/{variant}/{ISO_SHORT[iso]}/{a_lock}/{b_action}/{end}"
                        cells.append(cell(cid, "main", variant, iso,
                                          {"a_lock": a_lock, "b_action": b_action, "a_end": end},
                                          [step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                                           step("A", A_LOCKS[a_lock], "A.lock"),
                                           step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                                           step("B", B_ACTIONS[b_action], "B.action"),
                                           step("A", end_sql(end), "A.end"),
                                           step("B", "COMMIT", "B.end")],
                                          ["B.action"],
                                          "A and B at the same isolation level; B's snapshot is taken by B.action, after A.lock and before A ends."))

    # 2. B references (or re-inserts) a parent row that A inserted and has not committed.
    uncommitted = {
        "insert_child_of_new": "INSERT INTO workspaces (id, owner_id, name) VALUES (20, 3, 'w20')",
        "insert_same_key": "INSERT INTO users (id, name) VALUES (3, 'b3')",
        "insert_same_key_do_nothing": "INSERT INTO users (id, name) VALUES (3, 'b3') ON CONFLICT (id) DO NOTHING RETURNING id",
        "insert_same_key_do_update": "INSERT INTO users (id, name) VALUES (3, 'b3') ON CONFLICT (id) DO UPDATE SET name = excluded.name RETURNING id, name",
        "for_key_share_new": "SELECT id FROM users WHERE id = 3 FOR KEY SHARE",
    }
    for iso in ISOLATIONS:
        for b_action, sql in uncommitted.items():
            for end in ("commit", "rollback"):
                cid = f"uncommitted-parent/base/{ISO_SHORT[iso]}/{b_action}/{end}"
                cells.append(cell(cid, "uncommitted-parent", "base", iso,
                                  {"a_action": "insert_parent_3", "b_action": b_action, "a_end": end},
                                  [step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("A", "INSERT INTO users (id, name) VALUES (3, 'a3')", "A.insert"),
                                   step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("B", sql, "B.action"),
                                   step("A", end_sql(end), "A.end"),
                                   step("B", "COMMIT", "B.end")],
                                  ["B.action"]))

    # 3. B holds an uncommitted child insert (FOR KEY SHARE on the parent); A acts on the parent; B ends.
    held = [("base", a) for a in ["for_update", "for_no_key_update", "for_share", "for_key_share",
                                  "update_nonkey", "update_key_id", "delete"]]
    held += [("email", "update_key_email"), ("email", "update_key_email_same"), ("cascade", "delete")]
    for variant, a_action in held:
        for iso in ISOLATIONS:
            for end in ("commit", "rollback"):
                cid = f"held-child-insert/{variant}/{ISO_SHORT[iso]}/{a_action}/{end}"
                cells.append(cell(cid, "held-child-insert", variant, iso,
                                  {"b_holds": "insert_child", "a_action": a_action, "b_end": end},
                                  [step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("B", B_ACTIONS["insert_child"], "B.insert"),
                                   step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("A", A_LOCKS[a_action], "A.action"),
                                   step("B", end_sql(end), "B.end"),
                                   step("A", "COMMIT", "A.end")],
                                  ["A.action"],
                                  "B's insert is uncommitted while A acts on the referenced parent."))

    # 4. A commits a change to the parent AFTER B's snapshot, then B acts.
    post = [("base", a) for a in ["update_nonkey", "update_key_id", "delete", "for_update", "for_key_share"]]
    post += [("email", "update_key_email"), ("email", "update_key_email_same")]
    for variant, a_action in post:
        for iso in ISOLATIONS:
            for b_action in B_ACTIONS_MAIN:
                cid = f"post-snapshot/{variant}/{ISO_SHORT[iso]}/{a_action}/{b_action}"
                cells.append(cell(cid, "post-snapshot", variant, iso,
                                  {"a_committed": a_action, "b_action": b_action},
                                  [step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("B", "SELECT count(*) FROM users", "B.snapshot"),
                                   step("A", A_LOCKS[a_action], "A.autocommit"),
                                   step("B", B_ACTIONS[b_action], "B.action"),
                                   step("B", "COMMIT", "B.end")],
                                  ["B.action"],
                                  "A runs in autocommit, so its change is committed before B.action but after B's snapshot."))

    # 5. DEFERRABLE INITIALLY DEFERRED foreign key.
    for iso in ISOLATIONS:
        for a_lock in ["for_update", "for_no_key_update", "for_share", "for_key_share", "update_nonkey",
                       "update_key_id", "delete"]:
            for end in ("commit", "rollback"):
                cid = f"deferred/{ISO_SHORT[iso]}/a-holds/{a_lock}/{end}"
                cells.append(cell(cid, "deferred", "deferred", iso,
                                  {"order": "a-holds", "a_lock": a_lock, "a_end": end},
                                  [step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("A", A_LOCKS[a_lock], "A.lock"),
                                   step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("B", B_ACTIONS["insert_child"], "B.insert"),
                                   step("B", "COMMIT", "B.commit"),
                                   step("A", end_sql(end), "A.end")],
                                  ["B.insert", "B.commit"],
                                  "The foreign-key check (and its FOR KEY SHARE lock) runs at B's COMMIT."))
        for end in ("commit", "rollback"):
            cid = f"deferred/{ISO_SHORT[iso]}/b-inserts-first/delete/{end}"
            cells.append(cell(cid, "deferred", "deferred", iso,
                              {"order": "b-inserts-first", "a_lock": "delete", "a_end": end},
                              [step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                               step("B", B_ACTIONS["insert_child"], "B.insert"),
                               step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                               step("A", A_LOCKS["delete"], "A.delete"),
                               step("A", end_sql(end), "A.end"),
                               step("B", "COMMIT", "B.commit")],
                              ["A.delete", "A.end", "B.commit"],
                              "B's uncommitted child holds no lock on the parent until its COMMIT."))

    # 6. ON DELETE CASCADE: A deletes user 1 (cascading to workspace 11), B acts.
    for iso in ISOLATIONS:
        for b_action in ["insert_child", "update_child_fk", "for_key_share", "update_nonkey", "delete_parent",
                         "child_update_name", "child_for_key_share", "child_delete"]:
            for end in ("commit", "rollback"):
                cid = f"cascade/{ISO_SHORT[iso]}/delete/{b_action}/{end}"
                cells.append(cell(cid, "cascade", "cascade", iso,
                                  {"a_lock": "delete", "b_action": b_action, "a_end": end},
                                  [step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("A", A_LOCKS["delete"], "A.lock"),
                                   step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                                   step("B", B_ACTIONS[b_action], "B.action"),
                                   step("A", end_sql(end), "A.end"),
                                   step("B", "COMMIT", "B.end")],
                                  ["B.action"]))

    # 7. Foreign-key lock cycles: both insert a child of user 1, then both lock/update user 1.
    patterns = {
        "insert_then_for_update": "SELECT id FROM users WHERE id = 1 FOR UPDATE",
        "insert_then_for_no_key_update": "SELECT id FROM users WHERE id = 1 FOR NO KEY UPDATE",
        "insert_then_for_share": "SELECT id FROM users WHERE id = 1 FOR SHARE",
        "insert_then_update_nonkey": "UPDATE users SET name = name || '+' WHERE id = 1",
    }
    for iso in ISOLATIONS:
        for name, then_sql in patterns.items():
            cid = f"fk-cycle/{ISO_SHORT[iso]}/{name}"
            cells.append(cell(cid, "fk-cycle", "base", iso, {"pattern": name},
                              [step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                               step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                               step("A", "INSERT INTO workspaces (id, owner_id, name) VALUES (20, 1, 'w20')", "A.insert"),
                               step("B", "INSERT INTO workspaces (id, owner_id, name) VALUES (21, 1, 'w21')", "B.insert"),
                               step("A", then_sql, "A.then"),
                               step("B", then_sql, "B.then"),
                               step("A", "COMMIT", "A.end"),
                               step("B", "COMMIT", "B.end")],
                              ["A.then", "B.then", "A.end", "B.end"],
                              "Ground truth for eval E05/E06 (insert a child, then lock or update the parent)."))
        cid = f"fk-cycle/{ISO_SHORT[iso]}/lock_first_for_update"
        cells.append(cell(cid, "fk-cycle", "base", iso, {"pattern": "lock_first_for_update"},
                          [step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                           step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                           step("A", "SELECT id FROM users WHERE id = 1 FOR UPDATE", "A.then"),
                           step("B", "SELECT id FROM users WHERE id = 1 FOR UPDATE", "B.then"),
                           step("A", "INSERT INTO workspaces (id, owner_id, name) VALUES (20, 1, 'w20')", "A.insert"),
                           step("B", "INSERT INTO workspaces (id, owner_id, name) VALUES (21, 1, 'w21')", "B.insert"),
                           step("A", "COMMIT", "A.end"),
                           step("B", "COMMIT", "B.end")],
                          ["B.then", "A.end", "B.end"]))

    # 8. Check-then-insert of a child under a lock on the parent (eval E01-E04).
    guard_locks = {
        "none": "SELECT id FROM users WHERE id = 1",
        "for_update": "SELECT id FROM users WHERE id = 1 FOR UPDATE",
        "for_no_key_update": "SELECT id FROM users WHERE id = 1 FOR NO KEY UPDATE",
        "for_share": "SELECT id FROM users WHERE id = 1 FOR SHARE",
        "for_key_share": "SELECT id FROM users WHERE id = 1 FOR KEY SHARE",
    }
    check = "SELECT count(*) FROM workspaces WHERE owner_id = 1"
    for iso in ISOLATIONS:
        for mode, lock_sql in guard_locks.items():
            cid = f"guard/{ISO_SHORT[iso]}/{mode}"
            cells.append(cell(cid, "guard", "base", iso, {"lock": mode},
                              [step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                               step("A", lock_sql, "A.lock"),
                               step("A", check, "A.check"),
                               step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                               step("B", lock_sql, "B.lock"),
                               step("B", check, "B.check"),
                               step("A", "INSERT INTO workspaces (id, owner_id, name) VALUES (20, 1, 'w20')", "A.insert",
                                    {"label": "A.check", "firstValueEquals": "0"}),
                               step("A", "COMMIT", "A.end"),
                               step("B", "INSERT INTO workspaces (id, owner_id, name) VALUES (21, 1, 'w21')", "B.insert",
                                    {"label": "B.check", "firstValueEquals": "0"}),
                               step("B", "COMMIT", "B.end")],
                              ["B.lock", "B.check", "B.insert", "B.end"],
                              "Each session locks user 1, counts its workspaces, inserts one if none. "
                              "B's next statement runs as soon as its previous one returns."))
    # Mixed: B checks without a lock, A runs the locked check-then-insert, B inserts while A holds the lock.
    cells.append(cell("guard/rc/mixed_unlocked_vs_for_update", "guard", "base", "READ COMMITTED",
                      {"lock": "mixed: B none (autocommit), A for_update"},
                      [step("B", check, "B.check"),
                       step("A", "BEGIN ISOLATION LEVEL READ COMMITTED"),
                       step("A", guard_locks["for_update"], "A.lock"),
                       step("A", check, "A.check"),
                       step("A", "INSERT INTO workspaces (id, owner_id, name) VALUES (20, 1, 'w20')", "A.insert",
                            {"label": "A.check", "firstValueEquals": "0"}),
                       step("B", "INSERT INTO workspaces (id, owner_id, name) VALUES (21, 1, 'w21')", "B.insert",
                            {"label": "B.check", "firstValueEquals": "0"}),
                       step("A", "COMMIT", "A.end")],
                      ["B.insert"],
                      "The Phase 12 createWorkspace x createWorkspaceLocked divergence (eval E03)."))

    # 9. Concurrent inserts into a unique key containing NULL (eval E26).
    for parent in ("NULL", "5"):
        for end in ("commit", "rollback"):
            cid = f"nulls/rc/parent_{'null' if parent == 'NULL' else 'nonnull'}/{end}"
            cells.append(cell(cid, "nulls", "nulls", "READ COMMITTED", {"parent_id": parent, "a_end": end},
                              [step("A", "BEGIN"),
                               step("A", f"INSERT INTO folders (id, owner_id, parent_id, name) VALUES (1, 1, {parent}, 'a')", "A.insert"),
                               step("B", "BEGIN"),
                               step("B", f"INSERT INTO folders (id, owner_id, parent_id, name) VALUES (2, 1, {parent}, 'a')", "B.insert"),
                               step("A", end_sql(end), "A.end"),
                               step("B", "COMMIT", "B.end")],
                              ["B.insert"]))

    # 10. Upserts on an existing row (eval E22/E23).
    upsert = "INSERT INTO users (id, name) VALUES (1, '{v}') ON CONFLICT (id) DO UPDATE SET name = users.name || '+{v}' RETURNING id, name"
    cells.append(cell("upsert/rc/sequential_do_update_returning", "upsert", "base", "READ COMMITTED",
                      {"pattern": "two autocommitted upserts of an existing key, one after the other"},
                      [step("A", upsert.format(v="a"), "A.upsert"), step("B", upsert.format(v="b"), "B.upsert")],
                      ["A.upsert", "B.upsert"]))
    for iso in ISOLATIONS:
        for end in ("commit", "rollback"):
            cid = f"upsert/{ISO_SHORT[iso]}/concurrent_do_update_returning/{end}"
            cells.append(cell(cid, "upsert", "base", iso, {"pattern": "concurrent upserts of an existing key", "a_end": end},
                              [step("A", f"BEGIN ISOLATION LEVEL {iso}"),
                               step("A", upsert.format(v="a"), "A.upsert"),
                               step("B", f"BEGIN ISOLATION LEVEL {iso}"),
                               step("B", upsert.format(v="b"), "B.upsert"),
                               step("A", end_sql(end), "A.end"),
                               step("B", "COMMIT", "B.end")],
                              ["B.upsert"]))
    ids = [c["id"] for c in cells]
    assert len(ids) == len(set(ids)), "duplicate cell ids"
    return cells


# --------------------------------------------------------------------------
# Runner
# --------------------------------------------------------------------------

WAITS_SQL = """
SELECT l.locktype, l.mode, l.relation::regclass::text, l.page, l.tuple, l.transactionid::text,
       l.virtualxid, pg_blocking_pids(l.pid)::text, a.wait_event_type, a.wait_event
FROM pg_locks l LEFT JOIN pg_stat_activity a ON a.pid = l.pid
WHERE l.pid = {pid} AND NOT l.granted
"""

ROWLOCKS_SQL = "SELECT locked_row::text, multi, xids::text, modes::text, pids::text FROM pgrowlocks('{table}')"


def parse_pg_array(text):
    if not text or text == "{}":
        return []
    return [x.strip('"') for x in text.strip("{}").split(",")]


class Inconclusive(Exception):
    pass


class CellRun:
    def __init__(self, cluster: Cluster, obs: Conn, spec: dict, server_version: str):
        self.cluster, self.obs, self.spec = cluster, obs, spec
        self.server_version = server_version
        self.sessions = {}
        self.pidmap = {}
        self.pending = {}  # session -> step record whose statement is in flight
        self.queue = collections.defaultdict(collections.deque)
        self.labels = {}
        self.steps_out = []
        self.current_index = -1
        self.inconclusive = None

    # -- observation ------------------------------------------------------
    def waits(self, pid):
        r = self.obs.query(WAITS_SQL.format(pid=pid))
        out = []
        for row in r.rows:
            locktype, mode, rel, page, tup, xid, vxid, blockers, wtype, wevent = row
            out.append({k: v for k, v in {
                "locktype": locktype, "mode": mode, "relation": rel, "page": page, "tuple": tup,
                "transactionid": xid, "virtualxid": vxid,
                "blocked_by": [self.pidmap.get(int(p), f"pid {p}") for p in parse_pg_array(blockers)],
                "wait_event": f"{wtype}:{wevent}" if wtype else None,
            }.items() if v not in (None, "")})
        return out

    def rowlocks(self):
        snap = {}
        for table in self.spec["tables"]:
            r = self.obs.query(ROWLOCKS_SQL.format(table=table))
            if r.error:
                snap[table] = {"error": r.error.get("message")}
                continue
            rows = []
            for locked_row, multi, xids, modes, pids in r.rows:
                rows.append({"ctid": locked_row, "multi": multi == "t", "modes": parse_pg_array(modes),
                             "holders": [self.pidmap.get(int(p), f"pid {p}") for p in parse_pg_array(pids)]})
            if rows:
                snap[table] = rows
        return snap

    # -- execution --------------------------------------------------------
    def settle(self, s):
        """Wait until `s`'s statement completes or is seen waiting on a lock."""
        conn = self.sessions[s]
        end = time.monotonic() + SETTLE_TIMEOUT
        while True:
            r = conn.poll(0.001)
            if r is not None:
                return "done", r, None
            w = self.waits(conn.pid)
            if w:
                r = conn.poll(0)
                return ("done", r, w) if r is not None else ("waiting", None, w)
            if time.monotonic() > end:
                return "timeout", None, None

    def condition_holds(self, when):
        if not when:
            return True
        ref = self.labels.get(when["label"])
        if not ref or ref.get("status") != "done" or "result" not in ref:
            return False
        rows = ref["result"].get("rows") or []
        first = rows[0][0] if rows and rows[0] else None
        return first == when["firstValueEquals"]

    def execute(self, s, st, rec):
        if not self.condition_holds(st.get("when")):
            rec["status"] = "skipped"
            rec["reason"] = f"condition {st['when']} is false"
            return
        rec["sent_after_step"] = self.current_index
        self.sessions[s].send(st["sql"])
        status, r, w = self.settle(s)
        if w:
            rec["blocked"] = True
            rec["waits"] = w
        else:
            rec["blocked"] = False
        if status == "done":
            self.finish(rec, r)
            rec["completed_after_step"] = self.current_index
        elif status == "waiting":
            rec["status"] = "waiting"
            self.pending[s] = rec
        else:
            rec["status"] = "timeout"
            raise Inconclusive(f"step {rec['index']} ({s}: {st['sql']}) neither completed nor waited within {SETTLE_TIMEOUT}s")
        rec["rowlocks_after"] = self.rowlocks()

    def finish(self, rec, r):
        rec["status"] = "done"
        rec["result"] = r.to_json()

    def run_queue(self, s):
        while self.queue[s] and s not in self.pending:
            st, rec = self.queue[s].popleft()
            self.execute(s, st, rec)

    def resettle(self):
        changed = True
        while changed:
            changed = False
            for s in list(self.pending):
                status, r, w = self.settle(s)
                if status == "done":
                    rec = self.pending.pop(s)
                    self.finish(rec, r)
                    rec["completed_after_step"] = self.current_index
                    self.run_queue(s)
                    changed = True
                elif status == "timeout":
                    raise Inconclusive(f"blocked session {s} neither completed nor waited within {SETTLE_TIMEOUT}s")

    def drain(self):
        end = time.monotonic() + DRAIN_TIMEOUT
        while self.pending or any(self.queue.values()):
            progressed = False
            for s in list(self.pending):
                r = self.sessions[s].poll(0.002)
                if r is not None:
                    rec = self.pending.pop(s)
                    self.finish(rec, r)
                    rec["completed_after_step"] = "drain"
                    self.run_queue(s)
                    progressed = True
            for s in [s for s, q in self.queue.items() if q and s not in self.pending]:
                self.run_queue(s)
                progressed = True
            if not progressed and time.monotonic() > end:
                raise Inconclusive(f"sessions {sorted(self.pending)} still blocked {DRAIN_TIMEOUT}s after the schedule ended")

    def reset(self):
        for sql in self.spec["ddl"] + self.spec["seed"]:
            r = self.obs.query(sql)
            if r.error and not r.error.get("sqlstate", "").startswith("00"):
                raise PgError(f"setup failed: {sql}: {r.error}")

    def final_rows(self):
        out = {}
        for table in self.spec["tables"]:
            r = self.obs.query(f"SELECT * FROM {table} ORDER BY 1")
            out[table] = {"columns": r.columns, "rows": r.rows}
        return out

    def terminate_sessions(self):
        for conn in self.sessions.values():
            if conn.pid:
                self.obs.query(f"SELECT pg_terminate_backend({conn.pid})")
        for conn in self.sessions.values():
            try:
                conn.close()
            except OSError:
                pass

    def run(self):
        spec = self.spec
        out = {k: spec[k] for k in ("id", "family", "variant", "isolation", "params", "note") if spec.get(k)}
        out["server_version"] = self.server_version
        self.reset()
        names = []
        for st in spec["steps"]:
            if st["s"] not in names:
                names.append(st["s"])
        for name in names:
            conn = Conn(self.cluster.socket_path, f"plm-{name}")
            self.sessions[name] = conn
            self.pidmap[conn.pid] = name
        try:
            for idx, st in enumerate(spec["steps"]):
                self.current_index = idx
                rec = {"index": idx, "session": st["s"], "sql": st["sql"]}
                if st.get("label"):
                    rec["label"] = st["label"]
                    self.labels[st["label"]] = rec
                if st.get("when"):
                    rec["when"] = st["when"]
                self.steps_out.append(rec)
                s = st["s"]
                if s in self.pending or self.queue[s]:
                    rec["queued_behind_blocked_statement"] = True
                    self.queue[s].append((st, rec))
                else:
                    self.execute(s, st, rec)
                self.resettle()
            self.current_index = "drain"
            self.drain()
        except Inconclusive as exc:
            self.inconclusive = str(exc)
        except PgError as exc:
            self.inconclusive = f"client error: {exc}"
        if self.inconclusive:
            self.terminate_sessions()
        else:
            for name, conn in self.sessions.items():
                conn.close()
        out["steps"] = self.steps_out
        out["verdict"] = "INCONCLUSIVE" if self.inconclusive else "OK"
        if self.inconclusive:
            out["inconclusive_reason"] = self.inconclusive
        out["transactions"] = self.outcomes()
        out["focus"] = {lab: compact(self.labels.get(lab)) for lab in spec["focus"]}
        out["final"] = self.final_rows()
        return out

    def outcomes(self):
        result = {}
        for name in self.sessions:
            steps = [r for r in self.steps_out if r["session"] == name]
            began = any(r["sql"].upper().startswith("BEGIN") for r in steps)
            if not began:
                errs = [r for r in steps if r.get("result", {}).get("error")]
                result[name] = "autocommit: " + ("error " + ",".join(
                    r["result"]["error"].get("sqlstate", "?") for r in errs) if errs else "all statements ok")
                continue
            last = [r for r in steps if r["sql"].upper() in ("COMMIT", "ROLLBACK")]
            if not last or last[-1].get("status") != "done":
                result[name] = "unknown (did not end)"
                continue
            rec = last[-1]
            res = rec["result"]
            if res.get("error"):
                result[name] = f"failed at {rec['sql']} with {res['error'].get('sqlstate')}"
            elif rec["sql"].upper() == "ROLLBACK":
                result[name] = "rolled back (requested)"
            elif res.get("tag") == "ROLLBACK":
                errs = [r["result"]["error"].get("sqlstate") for r in steps
                        if r.get("result", {}).get("error")]
                result[name] = "rolled back at COMMIT (transaction had failed: " + ",".join(errs) + ")"
            else:
                result[name] = "committed"
        return result


def token(rec):
    """Short text for a step result: ok / ok:0 rows / SQLSTATE / skipped."""
    if rec is None:
        return "?"
    if rec.get("status") == "skipped":
        return "skipped"
    if rec.get("status") != "done":
        return rec.get("status", "?")
    res = rec["result"]
    if res.get("error"):
        return res["error"].get("sqlstate", "ERR")
    tag = res.get("tag") or ""
    parts = tag.split()
    if parts and parts[-1].isdigit() and parts[0] in ("UPDATE", "DELETE", "SELECT", "INSERT"):
        n = int(parts[-1])
        return "ok" if n > 0 else "ok(0 rows)"
    return "ok"


def compact(rec):
    if rec is None:
        return None
    out = {"blocked": bool(rec.get("blocked")), "result": token(rec)}
    if rec.get("waits"):
        w = rec["waits"][0]
        out["waited_on"] = {k: w[k] for k in ("locktype", "mode", "relation", "blocked_by", "wait_event") if k in w}
    if "completed_after_step" in rec:
        out["completed_after_step"] = rec["completed_after_step"]
    if rec.get("result", {}).get("error"):
        e = rec["result"]["error"]
        out["error"] = {k: e[k] for k in ("sqlstate", "message", "detail", "routine") if k in e}
    if rec.get("result", {}).get("rows"):
        out["rows"] = rec["result"]["rows"]
    return out


# --------------------------------------------------------------------------
# Summaries
# --------------------------------------------------------------------------


def main_lookup(results):
    table = {}
    for c in results:
        if c["family"] != "main":
            continue
        p = c["params"]
        table[(c["variant"], c["isolation"], p["a_lock"], p["b_action"], p["a_end"])] = c
    return table


def cell_text(commit_cell, rollback_cell):
    if not commit_cell or not rollback_cell:
        return "?"
    if "INCONCLUSIVE" in (commit_cell["verdict"], rollback_cell["verdict"]):
        return "INCONCLUSIVE"
    fc, fr = commit_cell["focus"]["B.action"], rollback_cell["focus"]["B.action"]
    wait = lambda f: ("W" + ("" if f.get("waited_on", {}).get("locktype") == "transactionid"
                             else f"[{f.get('waited_on', {}).get('locktype')}]")) if f["blocked"] else "-"
    w = wait(fc) if wait(fc) == wait(fr) else f"{wait(fc)}/{wait(fr)}"
    bc, br = b_outcome(commit_cell), b_outcome(rollback_cell)
    if fc["result"] == fr["result"] and bc == br:
        res = fc["result"] + bc
    else:
        res = f"C:{fc['result']}{bc} R:{fr['result']}{br}"
    return f"{w} {res}"


def b_outcome(c):
    t = c["transactions"].get("B", "")
    if t.startswith("failed at COMMIT"):
        return " +commit:" + t.split()[-1]
    return ""


def summary_markdown(results, meta):
    lines = []
    out = lines.append
    inconclusive = [c["id"] for c in results if c["verdict"] != "OK"]
    out("# Phase 13 lock matrix: observed PostgreSQL semantics\n")
    out(f"Generated by `scripts/phase13-lock-matrix.py` on {meta['generated_utc']}. "
        f"Server: PostgreSQL {meta['server_version']} (`{meta['version']}`). "
        f"Cells: {len(results)}; INCONCLUSIVE: {len(inconclusive)}.\n")
    out("Notation: `W` = B was observed waiting (ungranted lock in `pg_locks`; `W` alone = on a "
        "`transactionid`, `W[x]` = on lock type x); `-` = B did not wait. Then B's statement result: "
        "`ok`, `ok(0 rows)`, or the SQLSTATE. `C:` / `R:` split the result when it depends on whether A "
        "committed or rolled back; `+commit:X` = B's own COMMIT then failed with X.\n")
    look = main_lookup(results)
    for variant, locks in (("base", A_LOCKS_BASE), ("email", A_LOCKS_EMAIL)):
        for iso in ISOLATIONS:
            out(f"\n### main / {variant} / {iso}\n")
            out("| A holds \\ B does | " + " | ".join(B_ACTIONS_MAIN) + " |")
            out("|---|" + "---|" * len(B_ACTIONS_MAIN))
            for a in locks:
                row = [cell_text(look.get((variant, iso, a, b, "commit")), look.get((variant, iso, a, b, "rollback")))
                       for b in B_ACTIONS_MAIN]
                out(f"| {a} | " + " | ".join(row) + " |")
    # Common-description comparison
    out("\n### Blocking vs the common description\n")
    out("Expected = the manual's row-lock conflict table, reading a non-key UPDATE as FOR NO KEY UPDATE, "
        "a key UPDATE / DELETE as FOR UPDATE and a foreign-key check as FOR KEY SHARE.\n")
    mism = []
    agree = 0
    for c in results:
        if c["family"] != "main" or c["verdict"] != "OK":
            continue
        exp = common_blocks(c["params"]["a_lock"], c["params"]["b_action"])
        got = c["focus"]["B.action"]["blocked"]
        if exp is None:
            continue
        if exp == got:
            agree += 1
        else:
            mism.append(c["id"])
    out(f"Agreeing cells: {agree}; disagreeing: {len(mism)}.\n")
    for m in mism:
        out(f"- {m}")
    # Row-lock modes observed (pgrowlocks)
    out("\n### Row-lock modes observed with pgrowlocks after A's statement (main, READ COMMITTED)\n")
    out("| A statement | modes on users row 1 |")
    out("|---|---|")
    seen = {}
    for c in results:
        if c["family"] == "main" and c["isolation"] == "READ COMMITTED" and c["params"]["b_action"] == "for_key_share":
            a = c["params"]["a_lock"]
            step = next((s for s in c["steps"] if s.get("label") == "A.lock"), None)
            if step and a not in seen:
                locks = step.get("rowlocks_after", {}).get("users", [])
                seen[a] = ", ".join("/".join(r["modes"]) for r in locks) or "(none)"
    for a, m in seen.items():
        out(f"| {a} (`{A_LOCKS[a]}`) | {m} |")
    child = next((c for c in results if c["id"] == "held-child-insert/base/rc/for_key_share/commit"), None)
    if child:
        st = next(s for s in child["steps"] if s.get("label") == "B.insert")
        locks = st.get("rowlocks_after", {}).get("users", [])
        out(f"| child INSERT (`{B_ACTIONS['insert_child']}`) | "
            + (", ".join("/".join(r["modes"]) for r in locks) or "(none)") + " |")
    # Other families
    for fam in ("uncommitted-parent", "held-child-insert", "post-snapshot", "deferred", "cascade", "fk-cycle",
                "guard", "nulls", "upsert"):
        cells = [c for c in results if c["family"] == fam]
        if not cells:
            continue
        out(f"\n### {fam}\n")
        out("| cell | focus steps (blocked? result) | transactions | verdict |")
        out("|---|---|---|---|")
        for c in cells:
            foc = "; ".join(
                f"{lab}: {'W' if f and f['blocked'] else '-'} {f['result'] if f else '?'}"
                + (f" rows={f['rows']}" if f and f.get('rows') and fam in ('upsert', 'guard', 'uncommitted-parent') else "")
                for lab, f in c["focus"].items())
            tx = ", ".join(f"{k}: {v}" for k, v in c["transactions"].items())
            extra = ""
            if fam == "guard":
                ws = c["final"].get("workspaces", {}).get("rows", [])
                extra = f" — owner 1 workspaces: {sum(1 for r in ws if r[1] == '1')}"
            out(f"| {c['id']} | {foc}{extra} | {tx} | {c['verdict']} |")
    if inconclusive:
        out("\n### INCONCLUSIVE cells\n")
        for c in results:
            if c["verdict"] != "OK":
                out(f"- {c['id']}: {c.get('inconclusive_reason')}")
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--pg-bin", default=DEFAULT_PG_BIN)
    ap.add_argument("--from-cells", help="run the cells of this file instead of generating them")
    ap.add_argument("--only", action="append", default=[], help="run only cells whose id starts with this prefix")
    ap.add_argument("--write-cells-only", action="store_true")
    ap.add_argument("--results", default=str(RESULTS_PATH))
    ap.add_argument("--summary", default=str(SUMMARY_PATH))
    args = ap.parse_args()

    if args.from_cells:
        cells_doc = json.loads(Path(args.from_cells).read_text())
        cells = cells_doc["cells"]
    else:
        cells = generate_cells()
        cells_doc = {
            "schemaVersion": 1,
            "kind": "commitrift.phase13.lock-matrix.cells",
            "description": "PostgreSQL row-lock / foreign-key cells. Each cell: run `ddl` then `seed` (autocommit), then the "
                           "`steps` in order, each on its session (A, B: separate connections). A step whose session is "
                           "still blocked runs as soon as that session's previous statement returns. A step with `when` "
                           "runs only if the first value returned by the labeled step equals `firstValueEquals`. "
                           "`focus` names the steps whose blocking and result are the cell's subject.",
            "generator": "scripts/phase13-lock-matrix.py",
            "variants": {k: v["description"] for k, v in VARIANTS.items()},
            "a_locks": A_LOCKS,
            "b_actions": B_ACTIONS,
            "cells": cells,
        }
        CELLS_PATH.parent.mkdir(parents=True, exist_ok=True)
        CELLS_PATH.write_text(json.dumps(cells_doc, indent=1) + "\n")
        print(f"wrote {CELLS_PATH.relative_to(REPO)} ({len(cells)} cells)")
        if args.write_cells_only:
            return
    if args.only:
        cells = [c for c in cells if any(c["id"].startswith(p) for p in args.only)]

    cluster = Cluster(args.pg_bin)
    atexit.register(cluster.stop)
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda *_: sys.exit(130))
    t0 = time.monotonic()
    try:
        cluster.start()
        psql_version = cluster.psql(DATABASE, "SHOW server_version")
        obs = Conn(cluster.socket_path, "plm-observer")
        server_version = obs.query("SHOW server_version").first()
        version = obs.query("SELECT version()").first()
        version_num = obs.query("SHOW server_version_num").first()
        assert server_version == psql_version
        r = obs.query("CREATE EXTENSION IF NOT EXISTS pgrowlocks")
        if r.error:
            raise PgError(f"pgrowlocks: {r.error}")
        results = []
        for i, spec in enumerate(cells):
            run = CellRun(cluster, obs, spec, server_version)
            results.append(run.run())
            # nothing may survive a cell
            left = obs.query("SELECT pid FROM pg_stat_activity WHERE backend_type = 'client backend' "
                             "AND pid <> pg_backend_pid()")
            for (pid,) in left.rows:
                obs.query(f"SELECT pg_terminate_backend({pid})")
            if (i + 1) % 100 == 0:
                print(f"  {i + 1}/{len(cells)} cells", flush=True)
        obs.close()
    finally:
        cluster.stop()
    elapsed = time.monotonic() - t0
    meta = {
        "server_version": server_version, "server_version_num": version_num, "version": version,
        "pg_bin": args.pg_bin, "generated_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "settings": {"deadlock_timeout": DEADLOCK_TIMEOUT, "settle_timeout_s": SETTLE_TIMEOUT,
                     "drain_timeout_s": DRAIN_TIMEOUT, "fsync": "off", "listen_addresses": "''"},
        "elapsed_s": round(elapsed, 1),
        "cells": len(results),
        "inconclusive": sum(1 for c in results if c["verdict"] != "OK"),
    }
    header = {"schemaVersion": 1, "kind": "commitrift.phase13.lock-matrix.results", **meta,
              "cells_file": str(CELLS_PATH.relative_to(REPO)) if not args.from_cells else args.from_cells}
    # One JSON object per cell, one cell per line.
    head = json.dumps(header, indent=1)[:-2]
    body = ",\n".join(json.dumps(c, separators=(",", ":")) for c in results)
    Path(args.results).parent.mkdir(parents=True, exist_ok=True)
    Path(args.results).write_text(f'{head},\n "cells": [\n{body}\n ]\n}}\n')
    Path(args.summary).write_text(summary_markdown(results, meta))
    print(f"{len(results)} cells in {elapsed:.1f}s; INCONCLUSIVE: {meta['inconclusive']}; "
          f"PostgreSQL {server_version}")
    print(f"wrote {args.results} and {args.summary}")


if __name__ == "__main__":
    main()
