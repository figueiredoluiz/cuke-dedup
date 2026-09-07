#!/usr/bin/env node

import { run } from "../lib/launcher.mjs";

process.exitCode = run(process.argv.slice(2));
