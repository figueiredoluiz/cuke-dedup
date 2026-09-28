import { Given } from "@cucumber/cucumber";

// Purpose: a tag, data table and doc string are attachments, while keyword lines remain steps.
Given("the basket is ready", () => ready());
Given("the missing basket is ready", () => missing());
