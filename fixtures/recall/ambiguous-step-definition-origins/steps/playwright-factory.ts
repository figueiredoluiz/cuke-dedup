// The playwright-bdd factory hands back registrations from a call, not from the import binding.
import { createBdd } from "playwright-bdd";
const { Given } = createBdd();

Given("the kappa gear meshes {word}", () => j1());
Given("the kappa gear meshes cleanly", () => j2());
