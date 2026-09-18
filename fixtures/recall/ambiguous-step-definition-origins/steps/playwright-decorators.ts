// Decorator registrations come from an exact subpath and attach to class methods.
import { Given, Step } from "playwright-bdd/decorators";

export default class Fixtures {
  @Given("the lambda seal closes {word}")
  async lambdaParameterized(word: string) {
    await l1(word);
  }

  @Given("the lambda seal closes flush")
  async lambdaLiteral() {
    await l2();
  }

  // `Step` is a decorator-only export on this subpath.
  @Step("the mu spring loads {word}")
  async muParameterized(word: string) {
    await m1(word);
  }

  @Step("the mu spring loads slowly")
  async muLiteral() {
    await m2();
  }
}
