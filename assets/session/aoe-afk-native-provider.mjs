// The fixed provider has no action branch and no transport.
export function fixedProvider(Stream, config, state, { save, fault, native }) {
  const model = config.model;
  const base = () => ({ role: "assistant", api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(),
    content: [], stopReason: "stop", usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } });
  return (selected, _context, options) => {
    if (selected.id !== model.id || selected.provider !== model.provider || ++state.calls > 2 || !options.signal || options.signal.aborted) {
      fault(); throw Error("unexpected native fixture provider invocation");
    }
    const stream = new Stream(), message = base(), call = state.calls;
    let ended = false;
    const finishAbort = () => {
      if (ended) return;
      ended = true;
      state.cancelled = true;
      message.stopReason = "aborted"; message.errorMessage = "native diagnostic controlled cancellation";
      stream.push({ type: "error", reason: "aborted", error: message }); stream.end(message);
    };
    if (call === 2) {
      const episode = { signal: options.signal, consumed: false };
      state.episode = episode;
      const originalResult = stream.result.bind(stream);
      stream.result = async () => { const result = await originalResult(); episode.consumed = result.stopReason === "aborted"; return result; };
    }
    options.signal.addEventListener("abort", finishAbort, { once: true });
    void (async () => {
      try {
        const payload = { kind: "aoe-afk-native-v1", invocation: call };
        const replacement = await options.onPayload?.(payload, selected);
        if (replacement !== undefined && JSON.stringify(replacement) !== JSON.stringify(payload)) throw Error("unexpected payload replacement");
        await options.onResponse?.({ status: 200, headers: {} }, selected);
        if (ended) return;
        stream.push({ type: "start", partial: message });
        if (call === 2) {
          save("endpoint-native.json", native());
          save("pending.json", { calls: 2, active: true });
          return;
        }
        const tool = { type: "toolCall", id: "fixture-question-1", name: "ask_user", arguments: {} };
        message.content.push(tool);
        stream.push({ type: "toolcall_start", contentIndex: 0, partial: message });
        tool.arguments = config.question;
        stream.push({ type: "toolcall_delta", contentIndex: 0, delta: JSON.stringify(tool.arguments), partial: message });
        stream.push({ type: "toolcall_end", contentIndex: 0, toolCall: tool, partial: message });
        message.usage = config.ordinary_usage; message.stopReason = "toolUse";
        ended = true;
        options.signal.removeEventListener("abort", finishAbort);
        stream.push({ type: "done", reason: "toolUse", message }); stream.end(message);
      } catch (error) {
        fault();
        if (ended) return;
        ended = true;
        options.signal.removeEventListener("abort", finishAbort);
        message.stopReason = options.signal.aborted ? "aborted" : "error"; message.errorMessage = String(error);
        stream.push({ type: "error", reason: message.stopReason, error: message }); stream.end(message);
      }
    })();
    return stream;
  };
}
