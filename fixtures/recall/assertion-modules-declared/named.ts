import { Given } from "@cucumber/cucumber";
import { expect } from "./fixtures";
Given("the alpha meter shows a first value", () => expect(meter()).toBe(1));
Given("the alpha meter shows a second value", () => expect(meter()).toBe(2));
