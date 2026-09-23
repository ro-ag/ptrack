export interface TerminalSearchResult {
  resultIndex: number;
  resultCount: number;
}

export function terminalSearchResultLabel(
  result: TerminalSearchResult,
  hasQuery: boolean,
): string {
  if (!hasQuery) return "";
  if (result.resultCount === 0) return "No results";
  if (result.resultIndex < 0) return `${result.resultCount}+ results`;
  return `${result.resultIndex + 1} of ${result.resultCount}`;
}

/** Match highlighting shared by every terminal surface. */
export const terminalSearchDecorations = {
  matchBackground: "#26483e",
  matchBorder: "#3dd6a3",
  matchOverviewRuler: "#3dd6a3",
  activeMatchBackground: "#7a5f1f",
  activeMatchBorder: "#ffd75f",
  activeMatchColorOverviewRuler: "#ffd75f",
} as const;

export function terminalSearchOptions(incremental: boolean) {
  return { incremental, decorations: { ...terminalSearchDecorations } };
}
