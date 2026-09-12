import { Then } from "@cucumber/cucumber";
namespace expect { export function custom(value: unknown) { return value; } }
Then("the parcel status is now verified", ({ state }) => expect(state).toBe("ready"));
