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
