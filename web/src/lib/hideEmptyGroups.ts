import { sessionFlagGate } from "./sessionFlagGate";

const gate = sessionFlagGate("hide_empty_groups", true);

export const HideEmptyGroupsContext = gate.Context;
export const parseHideEmptyGroups = gate.parse;
export const useHideEmptyGroups = gate.use;
