// ESM named, namespace, and lowercase-alias imports are distinct resolution paths from CommonJS.
import { Given, given } from "@cucumber/cucumber";
import * as cucumber from "@cucumber/cucumber";

// ESM named import.
Given("the epsilon lamp glows {word}", () => e1());
Given("the epsilon lamp glows dim", () => e2());

// ESM namespace import plus member call.
cucumber.Given("the zeta rotor spins {word}", () => f1());
cucumber.Given("the zeta rotor spins clockwise", () => f2());

// A lowercase alias is a registration only because it was imported from a supported module.
given("the eta brake holds {word}", () => g1());
given("the eta brake holds firm", () => g2());
