#!/usr/bin/env python3
"""Write the Phase 10 first-party scenarios (fixtures/phase10/scenarios).

Each scenario runs one payment function of fixtures/phase10/ledger-live whose
recovery happens INSIDE the process that sees the failure (retry once after any
error), with a connection that breaks under the live process. Counterexample
scenarios have an invariant the blind recovery violates; probe scenarios have
an invariant that holds and ask the model for a schedule that contains the
loss (the schedule is the model's, never written by hand).
"""
import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "fixtures/phase10/scenarios")
INPUT = {"requestId": "req-1", "orderId": 1, "amount": 500}


def scenario(name, description, entry, table, invariant, bounds, probe=None, pool=None, harness="commitrift-source.json"):
    doc = {
        "schemaVersion": 1,
        "name": name,
        "description": "FIRST-PARTY APPLICATION (fixtures/phase10/ledger-live): " + description,
        "extraction": {
            "root": "../ledger-live",
            "externals": [],
            "schema": "src/lib/db/schema.ts",
        },
        "channels": {},
        "database": {
            "isolation": "read-committed",
            "tables": [{"from": table, "columns": (["id", "requestId", "orderId", "amount"])}],
            "initialState": {table: []},
        },
        "actors": [{
            "name": "Client",
            "entry": {"selector": f"src/app/payments.ts#{entry}", "invocation": "awaited"},
            "arguments": [INPUT],
            "description": "a request handler whose process survives a broken database connection and retries once",
        }],
        "bounds": {"maxSteps": 60, "maxCrashes": 0, "maxRetries": 0, "maxInFlight": 1, **bounds},
        "invariants": [invariant],
        "sourceVerification": {
            "harness": f"../ledger-live/{harness}",
            "actors": {"Client": {"entry": entry, "input": INPUT}},
        },
    }
    if pool:
        doc["pools"] = {"Client": pool}
    if probe:
        doc["sourceVerification"]["recoveryProbe"] = {"require": probe}
    return doc


def at_most_once(table, text):
    return {"name": "recorded-at-most-once", "when": "final", "description": text,
            "assert": {"le": [{"count": {"table": table, "where": {}}}, 1]}}


def at_least_once(table):
    return {"name": "recorded-at-least-once", "when": "final",
            "description": "the request ends up recorded (the retry did its job)",
            "assert": {"ge": [{"count": {"table": table, "where": {}}}, 1]}}


S = {}
S["live-blind-retry-lost-commit"] = scenario(
    "live-blind-retry-lost-commit",
    "the COMMIT reaches PostgreSQL, the connection breaks before the answer, the process lives, catches the error and records the payment again.",
    "recordPaymentRetryInProcess", "payments",
    at_most_once("payments", "one logical payment (one request id) is recorded at most once"), {})
S["live-blind-retry-commit-not-durable"] = scenario(
    "live-blind-retry-commit-not-durable",
    "the COMMIT never reaches PostgreSQL (the transaction rolls back), the connection breaks, the process lives and records the payment again: the application cannot tell this from the durable case, and here the retry is right.",
    "recordPaymentRetryInProcess", "payments", at_least_once("payments"), {},
    probe=["lost-not-committed:Client"])
S["live-autocommit-retry-lost-write"] = scenario(
    "live-autocommit-retry-lost-write",
    "an autocommit INSERT reaches PostgreSQL and commits by itself, the connection breaks before the answer, the process lives and inserts again.",
    "recordPaymentAutocommitRetryInProcess", "payments",
    at_most_once("payments", "one logical payment is recorded at most once"), {})
S["live-autocommit-retry-write-not-reached"] = scenario(
    "live-autocommit-retry-write-not-reached",
    "an autocommit INSERT never reaches PostgreSQL, the connection breaks, the process lives and inserts again.",
    "recordPaymentAutocommitRetryInProcess", "payments", at_least_once("payments"), {},
    probe=["lost-not-committed:Client"])
S["live-idempotent-retry-lost-commit"] = scenario(
    "live-idempotent-retry-lost-commit",
    "a durable unique request key (ON CONFLICT DO NOTHING) makes the in-process retry harmless when the COMMIT was durable.",
    "recordPaymentIdempotentRetryInProcess", "ledger",
    at_most_once("ledger", "one ledger row per request id"), {},
    probe=["lost-committed:Client"])
S["live-idempotent-retry-commit-not-durable"] = scenario(
    "live-idempotent-retry-commit-not-durable",
    "the same idempotent recovery when the COMMIT never reached PostgreSQL: the retry records the row once.",
    "recordPaymentIdempotentRetryInProcess", "ledger",
    at_most_once("ledger", "one ledger row per request id"), {},
    probe=["lost-not-committed:Client"])
S["live-reconciled-retry-lost-commit"] = scenario(
    "live-reconciled-retry-lost-commit",
    "after the error the process looks for the request and records the payment only when it is absent; the first COMMIT may reach PostgreSQL only after that look.",
    "recordPaymentReconciledRetryInProcess", "payments",
    at_most_once("payments", "one logical payment is recorded at most once"), {})
S["live-blind-retry-pool-of-one"] = scenario(
    "live-blind-retry-pool-of-one",
    "a connection pool of ONE connection: the broken connection is discarded and the retry obtains a new one (no starvation).",
    "recordPaymentRetryInProcess", "payments",
    at_most_once("payments", "one logical payment is recorded at most once"), {}, pool=1,
    harness="commitrift-source-pool1.json")

S["live-blind-retry-unhandled-error-event"] = scenario(
    "live-blind-retry-unhandled-error-event",
    "the same blind in-process retry under a process entry point that attaches NO error handler to checked-out clients: node-postgres emits `error` when the connection breaks, the event is unhandled and the Node process ends. The model predicts a live process; the run is expected to DIVERGE, and records why.",
    "recordPaymentRetryInProcess", "payments",
    at_most_once("payments", "one logical payment is recorded at most once"), {},
    harness="commitrift-source-unhandled.json")

# Other isolation levels (the application's transactions run at the
# database default).
for base, iso in [("live-blind-retry-lost-commit", "repeatable-read"),
                  ("live-blind-retry-lost-commit", "serializable"),
                  ("live-reconciled-retry-lost-commit", "serializable")]:
    doc = json.loads(json.dumps(S[base]))
    name = f"{base}-{iso}"
    doc["name"] = name
    doc["database"]["isolation"] = iso
    doc["description"] += f" Database default isolation: {iso.upper()}."
    S[name] = doc

# Two request handlers in ONE process (pool of two): one COMMIT loses its
# connection while the other request carries on in the same process.
shared = json.loads(json.dumps(S["live-blind-retry-lost-commit"]))
shared["name"] = "live-shared-process-lost-commit"
shared["description"] = ("FIRST-PARTY APPLICATION (fixtures/phase10/ledger-live): two request handlers run as tasks of "
                         "ONE process with a pool of two connections; one handler's COMMIT loses its connection, the "
                         "process and the other handler go on, the first retries in process.")
second = dict(shared["actors"][0], name="Other",
              arguments=[{"requestId": "req-2", "orderId": 2, "amount": 300}])
shared["actors"][0]["process"] = "P"
second["process"] = "P"
shared["actors"].append(second)
shared["pools"] = {"P": 2}
shared["invariants"] = [at_most_once("payments", "two requests record at most two payments")]
shared["invariants"][0]["assert"]["le"][1] = 2
shared["sourceVerification"]["harness"] = "../ledger-live/commitrift-source-pool2.json"
shared["sourceVerification"]["actors"]["Other"] = {"entry": "recordPaymentRetryInProcess",
                                                   "input": {"requestId": "req-2", "orderId": 2, "amount": 300}}
S["live-shared-process-lost-commit"] = shared

# postgres-js twins: the same application code on the other driver.
for base in ["live-blind-retry-lost-commit", "live-blind-retry-commit-not-durable",
             "live-autocommit-retry-lost-write", "live-autocommit-retry-write-not-reached",
             "live-idempotent-retry-lost-commit", "live-reconciled-retry-lost-commit"]:
    doc = json.loads(json.dumps(S[base]))
    name = base.replace("live-", "pgjs-", 1)
    doc["name"] = name
    doc["description"] = doc["description"].replace("fixtures/phase10/ledger-live", "fixtures/phase10/ledger-pgjs (postgres-js 3.4.5)")
    doc["extraction"]["root"] = "../ledger-pgjs"
    doc["sourceVerification"]["harness"] = "../ledger-pgjs/commitrift-source.json"
    S[name] = doc

# Phase 9's process-crash in-flight scenarios on postgres-js (the worker is
# killed with the operation in flight; the retry is a new process).
for p9, name in [("ledger-blind-commit-in-flight", "pgjs-crash-blind-commit-in-flight"),
                 ("ledger-reconciled-commit-in-flight", "pgjs-crash-reconciled-commit-in-flight"),
                 ("ledger-idempotent-commit-not-reached", "pgjs-crash-idempotent-commit-not-reached")]:
    with open(os.path.join(ROOT, "fixtures/phase9/scenarios", p9 + ".json")) as f:
        doc = json.load(f)
    doc["name"] = name
    doc["description"] = doc["description"].replace("fixtures/phase9/ledger-app", "fixtures/phase10/ledger-pgjs (postgres-js 3.4.5)")
    doc["extraction"]["root"] = "../ledger-pgjs"
    doc["sourceVerification"]["harness"] = "../ledger-pgjs/commitrift-source.json"
    S[name] = doc

os.makedirs(OUT, exist_ok=True)
for name, doc in S.items():
    with open(os.path.join(OUT, name + ".json"), "w") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")
print("wrote", len(S), "scenarios to", OUT)
