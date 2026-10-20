import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { classifyDiagnostic } from "./aoe-afk-observer-diagnostic.mjs";
import scenarios from "./aoe-afk-observer-scenarios.json" with { type: "json" };

// Fixed controlled child only. There is no arbitrary executable or native-run option.
export async function runControlledFixture(scenario = "normal") {
  if (!scenarios.includes(scenario)) throw Error("Unknown controlled fixture scenario");
  const executable = process.env.AOE_FIXTURE_PYTHON ?? "python3";
  const script = fileURLToPath(new URL("./aoe-afk-observer-supervisor.py", import.meta.url));
  const observed = await new Promise(resolve => {
    const child = spawn(executable, ["-I", "-S", script, scenario], { shell: false,
      env: { PATH: process.env.PATH ?? "/usr/bin:/bin", AOE_FIXTURE_NODE: process.execPath }, stdio: ["ignore", "pipe", "ignore"] });
    const chunks = []; let bytes = 0, overflow = false, exit;
    child.stdout.on("data", chunk => {
      bytes += chunk.length;
      if (bytes <= 2 * 1024 * 1024) chunks.push(chunk); else overflow = true;
    });
    child.once("error", () => resolve({ error: "supervisor_failed" }));
    child.once("exit", (code, signal) => { exit = { code, signal }; });
    child.once("close", () => {
      if (!exit || exit.code !== 0 || exit.signal !== null || overflow) return resolve({ error: "supervisor_failed" });
      try { resolve(JSON.parse(Buffer.concat(chunks).toString("utf8"))); }
      catch { resolve({ error: "supervisor_failed" }); }
    });
  });
  if (!observed || observed.error) return {
    provenance: "reconstructed_controlled_fixture", endpoint_scope: "controlled_release_not_native",
    diagnostic: classifyDiagnostic({ scope: "observed_calls", completion: observed?.completion }),
    failures: [["spawn_failed", "setup_failed", "holder_spawn_failed"].includes(observed?.error) ? observed.error : "supervisor_failed"],
    process: observed?.process ?? { main_reaped: false, cleanup_complete: false }, retained_root: observed?.retained_root ?? null,
  };
  const diagnostic = classifyDiagnostic({ scope: "observed_calls", endpoint: observed.endpoint,
    completion: observed.completion, trace: observed.trace, probes: observed.probes });
  return { provenance: "reconstructed_controlled_fixture", endpoint_scope: "controlled_release_not_native", diagnostic,
    completion: observed.completion, process: observed.process, ingestion: observed.ingestion,
    events: observed.events, failures: observed.failures, retained_root: observed.retained_root };
}
