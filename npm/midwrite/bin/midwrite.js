#!/usr/bin/env node
// Launcher for the `midwrite` binary. npm installs exactly one of the
// platform packages listed in optionalDependencies (selected by their
// `os`/`cpu` fields); this script finds it and runs its binary with the
// same arguments, stdio and exit status.
"use strict";

const { spawnSync } = require("node:child_process");
const { accessSync, chmodSync, constants } = require("node:fs");
const { join, dirname } = require("node:path");

const PLATFORMS = {
  "darwin arm64": "@midwrite/cli-darwin-arm64",
  "darwin x64": "@midwrite/cli-darwin-x64",
  "linux arm64": "@midwrite/cli-linux-arm64",
  "linux x64": "@midwrite/cli-linux-x64",
  "win32 x64": "@midwrite/cli-win32-x64",
  "win32 arm64": "@midwrite/cli-win32-arm64",
};

function binaryPath() {
  const key = `${process.platform} ${process.arch}`;
  const name = PLATFORMS[key];
  if (!name) {
    fail(
      `midwrite has no prebuilt binary for ${key}. ` +
        `Supported: ${Object.keys(PLATFORMS).join(", ")}. ` +
        "Build it from source instead: https://github.com/astra-void/midwrite#install",
    );
  }
  let manifest;
  try {
    manifest = require.resolve(`${name}/package.json`);
  } catch {
    fail(
      `The platform package ${name} is not installed. It is an optional ` +
        "dependency of midwrite; reinstall without --omit=optional or " +
        "--no-optional.",
    );
  }
  const binary = join(
    dirname(manifest),
    "bin",
    process.platform === "win32" ? "midwrite.exe" : "midwrite",
  );
  if (process.platform === "win32") {
    return binary;
  }
  try {
    accessSync(binary, constants.X_OK);
  } catch {
    // Some package managers drop the executable bit when extracting.
    try {
      chmodSync(binary, 0o755);
    } catch {
      fail(`${binary} is not executable.`);
    }
  }
  return binary;
}

function fail(message) {
  process.stderr.write(`midwrite: ${message}\n`);
  process.exit(2);
}

const result = spawnSync(binaryPath(), process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  fail(result.error.message);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
} else {
  process.exit(result.status ?? 1);
}
