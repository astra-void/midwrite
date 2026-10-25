// Differential-test runner: runs `main()` of one program and prints the
// operations it performed, in order, as JSON. Run with
// `node --experimental-transform-types run.mjs <program.ts>`.
// @expect none
const program = await import(new URL(process.argv[2], import.meta.url).href);
const runtime = await import(new URL("./runtime.ts", import.meta.url).href);
await program.main();
process.stdout.write(JSON.stringify(runtime.log));
