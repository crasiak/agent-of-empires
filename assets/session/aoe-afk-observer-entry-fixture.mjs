import { delegationRuntime } from "./aoe-afk-runtime.mjs";
import { registerObserved } from "./aoe-afk-observer-fixture.mjs";

export default function (pi) {
  registerObserved(pi, delegationRuntime);
}
