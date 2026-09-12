import { Then } from "@cucumber/cucumber";
import direct, { default as check } from "expect";
Then("the parcel status is verified", ({ state }) => check(state).toBe("ready"));
Then("the parcel status is now verified", ({ state }) => check(state).toBe("idle"));
Then("the archive badge is visible", ({ state }) => check(state).toBeVisible());
Then("the archive badge is now visible", ({ state }) => direct(state).toBeVisible());
