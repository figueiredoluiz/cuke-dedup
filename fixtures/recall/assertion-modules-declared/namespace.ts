import { Given } from "@cucumber/cucumber";
import * as fixtures from "./fixtures";
Given("the beta dial holds a first entry", () => fixtures.expect(dial()).toBe(1));
Given("the beta dial holds a second entry", () => fixtures.expect(dial()).toBe(2));
