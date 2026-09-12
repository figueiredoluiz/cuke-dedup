import { Then } from "@cucumber/cucumber";
declare namespace expect { function custom(value: unknown): unknown; }
Then("the parcel status is verified", ({ state }) => expect(state).toBe("ready"));
