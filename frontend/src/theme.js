// Color theme resolution and persistence. The stored value wins when the
// user picked a theme explicitly; otherwise the OS preference is followed.

export const THEME_STORAGE_KEY = "ptrack-theme";

export function resolveTheme(stored, prefersLight) {
  if (stored === "light" || stored === "dark") return stored;
  return prefersLight ? "light" : "dark";
}

export function nextTheme(theme) {
  return theme === "light" ? "dark" : "light";
}

// initTheme applies the resolved theme to root.dataset.theme, follows OS
// changes while no explicit choice is stored, and reports every applied
// theme through onChange. Returns a controller whose setTheme() applies a
// stored preference ("system" clears the explicit choice) and whose toggle()
// switches to the opposite of the currently resolved theme.
export function initTheme({ root, storage, media, onChange }) {
  let explicit = storage.getItem(THEME_STORAGE_KEY);

  const apply = () => {
    const theme = resolveTheme(explicit, media.matches);
    root.dataset.theme = theme;
    onChange?.(theme);
  };

  media.addEventListener("change", apply);
  apply();

  return {
    get theme() {
      return resolveTheme(explicit, media.matches);
    },
    setTheme(preference) {
      explicit = preference === "light" || preference === "dark" ? preference : null;
      if (explicit) storage.setItem(THEME_STORAGE_KEY, explicit);
      else storage.removeItem?.(THEME_STORAGE_KEY);
      apply();
      return resolveTheme(explicit, media.matches);
    },
    toggle() {
      return this.setTheme(nextTheme(resolveTheme(explicit, media.matches)));
    },
  };
}
