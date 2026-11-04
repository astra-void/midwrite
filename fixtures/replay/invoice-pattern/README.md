# Isolated reproduction of a suspected pattern (midday)

This harness reproduces, in isolation, the code SHAPE of a MW1001 finding that
the Phase 2 review labeled a likely bug in midday-ai/midday at revision
`51587319f26a0ffaa9dfccab1920373cb65689b7`
(`apps/api/src/rest/routers/invoices.ts:487-499`, finding `34107b6f5fb66fb4`;
the same shape at `apps/api/src/mcp/tools/invoices.ts:1141-1152` and `:1606-1614`):

1. a shared query helper `updateInvoice(db, { id, status: "unpaid" })` in
   another module writes on the database handle it receives (auto-commit);
2. the route then enqueues the `generate-invoice` job with `triggerJob(...)`.

It is NOT midday's code and it does not run midday. The helper, the route and
the job client are re-written here from the shapes visible in the source, with
a fake job queue (the replay event log) instead of BullMQ/Redis. A runtime
result here shows that the PATTERN loses the job when the process dies between
the two operations; it does not show that midday's production deployment
experiences this, how often, or whether its users or operators repair it
(for example by re-sending the invoice from the dashboard).
