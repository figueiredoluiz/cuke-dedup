#!/usr/bin/env node
// Rust toolchain check: the `rust-toolchain.toml` pin is the `Cargo.toml` MSRV's release.
// Workflows read the values through `--print` so every consumer parses them once.
//   check-rust-toolchain.mjs [--root DIR]               check pin against MSRV
//   check-rust-toolchain.mjs [--root DIR] --print pin   print the pin (after the check)
//   check-rust-toolchain.mjs [--root DIR] --print msrv  print the MSRV release, x.y.z
// Exit 0 agree, 1 disagree or unreadable, 2 usage.
// No entry-point guard: `import.meta.url === argv[1]` fails under a symlinked or differently cased
// path and skips the check with exit 0. Tests import the lib and run this file as a subprocess.
import { main } from "./lib/rust-toolchain.mjs";

process.exitCode = main(process.argv.slice(2));
