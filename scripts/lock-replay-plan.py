#!/usr/bin/env python3
"""Translate the PostgreSQL lock-matrix ground truth (the results file written
by scripts/lock-matrix.py independently of the model) into model replays:
for each cell, the two sessions as declared programs (the scenario DSL), the
order of their steps, and what PostgreSQL did at each labeled step (waited
or not; row count or SQLSTATE).

Writes fixtures/model/lock-matrix/model-replay.json, which the model test
`lock_ground_truth` replays. Cells whose SQL the translation does not cover,
or whose behavior the model refuses (ON DELETE CASCADE, DEFERRABLE INITIALLY
DEFERRED, string concatenation, deadlock victim choice), are listed with the
reason, never dropped silently.

Usage: scripts/lock-replay-plan.py [RESULTS.json]
       (default reports/lock-matrix/results.json)
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(ROOT, "reports", "lock-matrix", "results.json")
CELLS = os.path.join(ROOT, "fixtures", "model", "lock-matrix", "cells.json")
OUT = os.path.join(ROOT, "fixtures", "model", "lock-matrix", "model-replay.json")


class Skip(Exception):
    pass


def lit(v):
    v = v.strip()
    if v.startswith("'") and v.endswith("'"):
        return json.dumps(v[1:-1])
    if re.fullmatch(r"-?\d+", v):
        return v
    raise Skip(f"value `{v}`")


def assigns(text):
    out = []
    for part in text.split(","):
        col, val = part.split("=", 1)
        val = val.strip()
        m = re.fullmatch(r"excluded\.(\w+)", val)
        out.append(f"@{col.strip()} = " + (f"excluded.{m.group(1)}" if m else lit(val)))
    return ", ".join(out)


def stmt(sql, var):
    s = sql.strip().rstrip(";")
    if m := re.fullmatch(r"BEGIN ISOLATION LEVEL (READ COMMITTED|REPEATABLE READ|SERIALIZABLE)", s):
        return "begin isolation " + m.group(1).lower().replace(" ", "-"), None
    if s in ("COMMIT", "ROLLBACK"):
        return s.lower(), None
    if m := re.fullmatch(r"SELECT count\(\*\) FROM (\w+)(?: WHERE (\w+) = (\S+))?", s):
        where = f"@{m.group(2)} == {lit(m.group(3))}" if m.group(2) else "@id >= 0"
        return f"let {var} = count {m.group(1)} where {where}", "aggregate"
    if m := re.fullmatch(r"SELECT [\w, ]+ FROM (\w+) WHERE (\w+) = (\S+)( FOR (UPDATE|NO KEY UPDATE|SHARE|KEY SHARE))?", s):
        lock = f" for {m.group(5).lower()}" if m.group(4) else ""
        return f"let {var} = select {m.group(1)} where @{m.group(2)} == {lit(m.group(3))}{lock}", "select"
    if m := re.fullmatch(r"UPDATE (\w+) SET (.+) WHERE (\w+) = (\S+)", s):
        return f"update {m.group(1)} where @{m.group(3)} == {lit(m.group(4))} set {assigns(m.group(2))} into {var}", "count"
    if m := re.fullmatch(r"DELETE FROM (\w+) WHERE (\w+) = (\S+)", s):
        return f"delete {m.group(1)} where @{m.group(2)} == {lit(m.group(3))} into {var}", "count"
    if m := re.fullmatch(r"INSERT INTO (\w+) \(([\w, ]+)\) VALUES \(([^)]*)\)(.*)", s):
        cols = [c.strip() for c in m.group(2).split(",")]
        vals = [lit(v) for v in m.group(3).split(",")]
        rest = m.group(4).strip()
        rest = re.sub(r" ?RETURNING .*$", "", rest).strip()
        tail = ""
        if rest == "ON CONFLICT (id) DO NOTHING":
            tail = " on conflict do nothing"
        elif mm := re.fullmatch(r"ON CONFLICT \((\w+)\) DO UPDATE SET (.+)", rest):
            tail = f" on conflict (@{mm.group(1)}) do update set {assigns(mm.group(2))}"
        elif rest:
            raise Skip(f"`{rest}`")
        sets = ", ".join(f"@{c} = {v}" for c, v in zip(cols, vals))
        return f"insert {m.group(1)} set {sets}{tail} into {var}", "count"
    raise Skip(f"SQL `{s}`")


def tables(cell):
    users = [{"name": "id", "notNull": True, "type": "int"}, {"name": "name", "type": "text"}]
    unique = []
    if any("email text UNIQUE" in d for d in cell["ddl"]):
        users.append({"name": "email", "type": "text"})
        unique = [["email"]]
    if any("CASCADE" in d.replace("DROP TABLE IF EXISTS folders, workspaces, users CASCADE", "") or "DEFERRABLE" in d for d in cell["ddl"]):
        raise Skip("a referential action or deferral the model refuses")
    if any("folders" in d and "CREATE" in d for d in cell["ddl"]):
        raise Skip("folders table")
    return [
        {"name": "users", "columns": users, "primaryKey": ["id"], "unique": unique},
        {"name": "workspaces", "columns": [{"name": "id", "notNull": True, "type": "int"},
                                           {"name": "owner_id", "type": "int"}, {"name": "name", "type": "text"}],
         "primaryKey": ["id"],
         "foreignKeys": [{"columns": ["owner_id"], "references": "users", "referencedColumns": ["id"]}]},
    ]


def seeds(cell):
    out = {"users": [], "workspaces": []}
    for s in cell["seed"]:
        m = re.fullmatch(r"INSERT INTO (\w+) \(([\w, ]+)\) VALUES (.+)", s.strip())
        if not m:
            raise Skip(f"seed `{s}`")
        cols = [c.strip() for c in m.group(2).split(",")]
        for tup in re.findall(r"\(([^)]*)\)", m.group(3)):
            vals = [json.loads(lit(v)) for v in tup.split(",")]
            out[m.group(1)].append(dict(zip(cols, vals)))
    return out


def outcome(step):
    r = step.get("result") or {}
    err = r.get("error")
    if err:
        return {"sqlstate": err["sqlstate"]}
    if step["sql"].startswith("SELECT count(*)"):
        return {"count": int(r["rows"][0][0])}
    tag = r.get("tag") or ""
    if m := re.fullmatch(r"(?:INSERT \d+|UPDATE|DELETE|SELECT) (\d+)", tag):
        return {"rows": int(m.group(1))}
    return {}


def main():
    if len(sys.argv) > 2 or (len(sys.argv) == 2 and sys.argv[1].startswith("-")):
        sys.exit(__doc__)
    results = json.load(open(sys.argv[1] if len(sys.argv) == 2 else RESULTS))
    defs = {c["id"]: c for c in json.load(open(CELLS))["cells"]}
    replays, skipped = [], []
    for res in results["cells"]:
        cell = defs[res["id"]]
        try:
            if res["family"] == "fk-cycle":
                raise Skip("deadlock victim choice (the model explores both victims)")
            if any("blocked" not in st for st in res["steps"]):
                raise Skip("conditional steps (application logic decides whether a statement is sent)")
            programs = {"A": [], "B": []}
            order = []
            for i, step in enumerate(res["steps"]):
                line, kind = stmt(step["sql"], f"r{i}")
                programs[step["session"]].append(line)
                order.append({
                    "sent": step.get("sent_after_step", i), "index": i,
                    "session": step["session"], "label": step.get("label"), "var": f"r{i}" if kind else None,
                    "kind": kind, "blocked": bool(step["blocked"]), "postgres": outcome(step),
                })
            # The order PostgreSQL received the statements: a session sends
            # its next statement only after the previous one completed.
            # `sent` is the step after which it was sent: a deferred step
            # goes right after that step.
            order.sort(key=lambda o: (o["sent"] + (0.5 if o["sent"] != o["index"] else 0), o["index"]))
            replays.append({"id": res["id"], "tables": tables(cell), "seed": seeds(cell),
                            "programs": {k: "\n".join(v) + "\n" for k, v in programs.items()}, "steps": order})
        except Skip as e:
            skipped.append({"id": res["id"], "reason": str(e)})
    doc = {"$comment": "GENERATED by scripts/lock-replay-plan.py from the PostgreSQL ground truth; do not edit.",
           "serverVersion": results["server_version"], "replays": replays, "skipped": skipped}
    with open(OUT, "w") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")
    print(f"{len(replays)} cells translated, {len(skipped)} skipped")
    import collections
    print(collections.Counter(s["reason"] for s in skipped))


if __name__ == "__main__":
    main()
