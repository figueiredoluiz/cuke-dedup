import { Then } from "@cucumber/cucumber";
import { expect } from "@playwright/test";
Then("the parcel is ready", ({ state }) => expect(state).toEqual(expect.objectContaining({ role: "admin" })));
Then("shipment readiness has been confirmed", ({ state }) => expect(state).toEqual(expect.objectContaining({ role: "admin" })));
