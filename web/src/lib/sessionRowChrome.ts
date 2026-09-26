// Class recipe for a session row's active and selected chrome, resolved in one place so the rings don't fight by stylesheet order.

/** The open row gets an inset `session-active` frame meeting the WCAG non-text contrast floor, which turns `text-primary` while its main panel has input focus; selection gets a thinner translucent ring. Hover is withheld from the open row. */
export function sessionRowChromeClass(isActive: boolean, isSelected: boolean, hasInputFocus = false): string {
  if (isActive) {
    const frame = `ring-2 ring-inset ${hasInputFocus ? "ring-text-primary" : "ring-session-active"}`;
    return isSelected ? `${frame} bg-brand-500/15` : frame;
  }
  return isSelected
    ? "ring-1 ring-inset ring-brand-500/60 bg-brand-500/10 hover:bg-surface-700/40"
    : "hover:bg-surface-700/40";
}
