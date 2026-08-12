(function () {
  "use strict";

  const root = document.documentElement;
  const themeButton = document.querySelector("[data-theme-toggle]");
  const themeKey = "ptrack-help-theme";

  function systemTheme() {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }

  function selectedTheme() {
    const saved = window.localStorage.getItem(themeKey);
    return saved === "light" || saved === "dark" ? saved : systemTheme();
  }

  function applyTheme(theme) {
    root.dataset.theme = theme;
    if (themeButton) {
      const next = theme === "dark" ? "light" : "dark";
      themeButton.textContent = theme === "dark" ? "☀" : "☾";
      themeButton.setAttribute("aria-label", `Use ${next} theme`);
      themeButton.title = `Use ${next} theme`;
    }
  }

  if (themeButton) {
    applyTheme(selectedTheme());
    themeButton.addEventListener("click", function () {
      const next = root.dataset.theme === "dark" ? "light" : "dark";
      window.localStorage.setItem(themeKey, next);
      applyTheme(next);
    });
  }

  const form = document.querySelector("[data-help-search]");
  if (!form) return;

  const input = form.querySelector("input[type='search']");
  const status = form.querySelector("[data-search-status]");
  const results = form.querySelector("[data-search-results]");
  const helpRoot = document.body.dataset.helpRoot || "./";
  const helpRootURL = new URL(helpRoot, window.location.href);
  let entries = [];

  function normalize(value) {
    return value.toLocaleLowerCase().normalize("NFKD");
  }

  function searchableText(entry) {
    return normalize(
      [entry.title, entry.summary, ...entry.headings, ...entry.keywords].join(" "),
    );
  }

  function searchEntry(entry) {
    if (
      !entry ||
      typeof entry.title !== "string" ||
      typeof entry.summary !== "string" ||
      typeof entry.url !== "string" ||
      !Array.isArray(entry.headings) ||
      !entry.headings.every((value) => typeof value === "string") ||
      !Array.isArray(entry.keywords) ||
      !entry.keywords.every((value) => typeof value === "string")
    ) {
      return null;
    }
    let target;
    try {
      target = new URL(entry.url, helpRootURL);
    } catch {
      return null;
    }
    if (target.origin !== helpRootURL.origin || !target.pathname.startsWith(helpRootURL.pathname)) {
      return null;
    }
    return { ...entry, target: target.href };
  }

  function clearResults(message) {
    results.replaceChildren();
    status.textContent = message;
  }

  function renderMatches(query) {
    const term = normalize(query.trim());
    if (!term) {
      clearResults("Type a word or phrase to search all Help Center guides.");
      return;
    }

    const matches = entries.filter((entry) => searchableText(entry).includes(term)).slice(0, 8);
    results.replaceChildren();

    for (const entry of matches) {
      const item = document.createElement("li");
      const link = document.createElement("a");
      const summary = document.createElement("span");
      link.href = entry.target;
      link.textContent = entry.title;
      summary.textContent = entry.summary;
      link.append(summary);
      item.append(link);
      results.append(item);
    }

    status.textContent = matches.length === 1 ? "1 result." : `${matches.length} results.`;
  }

  form.addEventListener("submit", function (event) {
    event.preventDefault();
    renderMatches(input.value);
    results.querySelector("a")?.focus();
  });

  input.addEventListener("input", function () {
    renderMatches(input.value);
  });

  fetch(helpRoot + "search-index.json", { credentials: "same-origin" })
    .then(function (response) {
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return response.json();
    })
    .then(function (index) {
      entries = Array.isArray(index.entries) ? index.entries.map(searchEntry).filter(Boolean) : [];
      status.textContent = "Type a word or phrase to search all Help Center guides.";
      input.disabled = false;
    })
    .catch(function () {
      clearResults("Search is unavailable. Use the guide links below.");
    });
})();
