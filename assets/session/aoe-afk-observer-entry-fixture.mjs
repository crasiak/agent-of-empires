import { delegationRuntime } from "./aoe-afk-runtime.mjs";
import { registerObserved } from "./aoe-afk-observer-fixture.mjs";
import { privateProbe, observeRefresh, observeBridge, observeDelegation } from "./aoe-afk-observer-probes.mjs";

export default function (pi) {
  registerObserved(pi, delegationRuntime, { probes: { open: privateProbe, refresh: observeRefresh, bridge: observeBridge, delegation: observeDelegation } });
}
