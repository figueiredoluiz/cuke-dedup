// ESM named, namespace, and lowercase-alias imports are distinct resolution paths from CommonJS,
// and so are a local rename, a default import and TypeScript's import-equals. A type-only import is
// erased at runtime and must never register.
import { Given, given } from "@cucumber/cucumber";
import * as cucumber from "@cucumber/cucumber";
import { Given as Renamed } from "@cucumber/cucumber";
import cucumberDefault from "@cucumber/cucumber";
import equals = require("@cucumber/cucumber");
import type { Given as TypeOnly } from "@cucumber/cucumber";

// ESM named import.
Given("the epsilon lamp glows {word}", () => e1());
Given("the epsilon lamp glows dim", () => e2());

// ESM namespace import plus member call.
cucumber.Given("the zeta rotor spins {word}", () => f1());
cucumber.Given("the zeta rotor spins clockwise", () => f2());

// A lowercase alias is a registration only because it was imported from a supported module.
given("the eta brake holds {word}", () => g1());
given("the eta brake holds firm", () => g2());

// A local rename of an ESM named import: the binding name carries no evidence, the import does.
Renamed("the rho cable tightens {word}", () => r1());
Renamed("the rho cable tightens fully", () => r2());

// A default import reached through a member call.
cucumberDefault.Given("the sigma wheel locks {word}", () => s1());
cucumberDefault.Given("the sigma wheel locks hard", () => s2());

// TypeScript's import-equals form.
equals.Given("the tau chain feeds {word}", () => t1());
equals.Given("the tau chain feeds evenly", () => t2());

// Control: a type-only import is erased before runtime, so it cannot register anything.
TypeOnly("the upsilon belt slips {word}", () => u1());
TypeOnly("the upsilon belt slips loose", () => u2());
