import { Then } from "@cucumber/cucumber";
import { expect } from "synthetic-assertion-library";
Then("the parcel status is now verified", ({ state }) => expect(state).toBe("ready"));
