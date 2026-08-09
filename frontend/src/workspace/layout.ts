export const defaultSidebarWidth = 248;
export const minimumSidebarWidth = 180;
export const maximumSidebarWidth = 420;
export const sidebarWidthStorageKey = "ptrack-sidebar-width";
export const sidebarHiddenStorageKey = "ptrack-sidebar-hidden";

export function sidebarMaximumWidth(viewportWidth: number): number {
  return Math.max(
    minimumSidebarWidth,
    Math.min(maximumSidebarWidth, Math.floor(viewportWidth * 0.45)),
  );
}

export function clampSidebarWidth(width: number, viewportWidth: number): number {
  const responsiveMaximum = sidebarMaximumWidth(viewportWidth);
  const finiteWidth = Number.isFinite(width) ? width : defaultSidebarWidth;
  return Math.round(
    Math.max(minimumSidebarWidth, Math.min(finiteWidth, responsiveMaximum)),
  );
}

export function storedSidebarWidth(
  value: string | null,
  viewportWidth: number,
): number {
  if (value === null || value.trim() === "") {
    return clampSidebarWidth(defaultSidebarWidth, viewportWidth);
  }
  return clampSidebarWidth(Number(value), viewportWidth);
}

export function sidebarWidthFromKey(
  currentWidth: number,
  key: string,
  viewportWidth: number,
): number | null {
  if (key === "ArrowLeft") return clampSidebarWidth(currentWidth - 16, viewportWidth);
  if (key === "ArrowRight") return clampSidebarWidth(currentWidth + 16, viewportWidth);
  if (key === "PageDown") return clampSidebarWidth(currentWidth - 64, viewportWidth);
  if (key === "PageUp") return clampSidebarWidth(currentWidth + 64, viewportWidth);
  if (key === "Home") return minimumSidebarWidth;
  if (key === "End") return clampSidebarWidth(maximumSidebarWidth, viewportWidth);
  return null;
}
