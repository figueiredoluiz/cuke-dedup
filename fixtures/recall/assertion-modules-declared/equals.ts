import { Given } from "@cucumber/cucumber";
import fixtures = require("./fixtures");
Given("the gamma probe records a first reading", () => fixtures.expect(probe()).toBe(1));
Given("the gamma probe records a second reading", () => fixtures.expect(probe()).toBe(2));
