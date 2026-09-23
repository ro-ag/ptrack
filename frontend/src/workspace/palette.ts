import {
  focusCycleIndex,
  groupSearchResults,
  paletteStatusPresentation,
  paletteTarget,
  type PaletteResult,
} from "./presentation";
import type { AppContext } from "./app-context";
import { element } from "./dom";

const paletteKindLabels: Record<PaletteResult["kind"], string> = {
  issue: "Issue",
  plan: "Plan",
  task: "Task",
  note: "Note",
};

export function paletteEmptyState(message: string): HTMLDivElement {
  const empty = document.createElement("div");
  empty.className = "palette-empty";
  empty.textContent = message;
  return empty;
}

function paletteStatusBadge(result: PaletteResult): HTMLSpanElement | null {
  const statusPresentation = paletteStatusPresentation(result);
  if (!statusPresentation) return null;
  const status = document.createElement("span");
  status.className = "palette-status";
  status.dataset.status = statusPresentation.tone;
  status.title = statusPresentation.label;
  const glyph = document.createElement("span");
  glyph.setAttribute("aria-hidden", "true");
  glyph.textContent = statusPresentation.glyph;
  const label = document.createElement("span");
  label.className = "visually-hidden";
  label.textContent = `${statusPresentation.label} status`;
  status.append(glyph, label);
  return status;
}

function paletteOptionBody(result: PaletteResult): HTMLDivElement {
  const body = document.createElement("div");
  body.className = "palette-option-body";
  const title = document.createElement("p");
  title.className = "palette-option-title";
  title.textContent =
    result.kind === "note" ? result.title : `#${result.id} ${result.title}`;
  body.append(title);
  if (result.snippet) {
    const snippet = document.createElement("p");
    snippet.className = "palette-option-snippet";
    snippet.textContent = result.snippet;
    body.append(snippet);
  }
  return body;
}

export function createPalette(ctx: AppContext) {
  const { api, showError, workspaceController } = ctx;
  const elements = {
    navIssues: element("#nav-issues", HTMLButtonElement),
    palette: element("#palette", HTMLDivElement),
    paletteInput: element("#palette-input", HTMLInputElement),
    paletteResults: element("#palette-results", HTMLDivElement),
  };

  let paletteItems: PaletteResult[] = [];
  let paletteActive = -1;
  let paletteTimer = 0;
  let paletteSequence = 0;
  let paletteReturnFocus: HTMLElement | null = null;

  function openPalette(): void {
    if (
      workspaceController.state.status !== "open" ||
      ctx.state.firstPlanState.phase !== "idle"
    ) return;
    const active = document.activeElement;
    paletteReturnFocus = active instanceof HTMLElement ? active : null;
    elements.palette.hidden = false;
    renderPaletteResults();
    if (elements.paletteInput.value.trim()) void runPaletteSearch();
    requestAnimationFrame(() => {
      elements.paletteInput.focus();
      elements.paletteInput.select();
    });
  }

  function closePalette(): void {
    if (elements.palette.hidden) return;
    window.clearTimeout(paletteTimer);
    paletteSequence += 1;
    ctx.snapshot.hideApplicationOverlay(elements.palette);
    paletteItems = [];
    paletteActive = -1;
    paletteReturnFocus?.focus?.();
    paletteReturnFocus = null;
  }

  function schedulePaletteSearch(): void {
    window.clearTimeout(paletteTimer);
    paletteTimer = window.setTimeout(() => void runPaletteSearch(), 150);
  }

  async function runPaletteSearch(): Promise<void> {
    const query = elements.paletteInput.value.trim();
    const request = ++paletteSequence;
    if (!query) {
      paletteItems = [];
      paletteActive = -1;
      renderPaletteResults();
      return;
    }
    try {
      const results = await api().SearchV2(query);
      if (request !== paletteSequence || elements.palette.hidden) return;
      paletteItems = results;
      paletteActive = results.length ? 0 : -1;
      renderPaletteResults();
    } catch (error) {
      if (request !== paletteSequence || elements.palette.hidden) return;
      showError(error);
    }
  }

  function renderPaletteResults(): void {
    elements.paletteResults.replaceChildren();
    if (!elements.paletteInput.value.trim()) {
      elements.paletteResults.append(
        paletteEmptyState("Search across plans, tasks, and memory notes."),
      );
      elements.paletteInput.removeAttribute("aria-activedescendant");
      return;
    }
    if (paletteItems.length === 0) {
      elements.paletteResults.append(paletteEmptyState("No matches."));
      elements.paletteInput.removeAttribute("aria-activedescendant");
      return;
    }
    let flatIndex = 0;
    groupSearchResults(paletteItems).forEach((group) => {
      const section = document.createElement("div");
      section.className = "palette-group";
      const label = document.createElement("p");
      label.className = "palette-group-label";
      label.textContent = group.label;
      section.append(label);
      group.items.forEach((result) => {
        const index = flatIndex;
        const option = document.createElement("div");
        option.className = "palette-option";
        option.id = `palette-option-${index}`;
        option.role = "option";
        option.setAttribute("aria-selected", String(index === paletteActive));
        if (index === paletteActive) option.classList.add("active");
        const badge = document.createElement("span");
        badge.className = "palette-kind";
        badge.dataset.kind = result.kind;
        badge.textContent = paletteKindLabels[result.kind] || result.kind;
        const status = paletteStatusBadge(result);
        option.append(badge);
        if (status) option.append(status);
        option.append(paletteOptionBody(result));
        option.addEventListener("click", () => activatePaletteResult(result));
        option.addEventListener("mousemove", () => {
          if (paletteActive !== index) {
            paletteActive = index;
            renderPaletteResults();
          }
        });
        section.append(option);
        flatIndex += 1;
      });
      elements.paletteResults.append(section);
    });
    const active = elements.paletteResults.querySelector(".palette-option.active");
    if (active) {
      elements.paletteInput.setAttribute("aria-activedescendant", active.id);
      active.scrollIntoView({ block: "nearest" });
    } else {
      elements.paletteInput.removeAttribute("aria-activedescendant");
    }
  }

  function movePaletteActive(delta: number): void {
    if (paletteItems.length === 0) return;
    paletteActive = focusCycleIndex(
      paletteItems.length,
      paletteActive,
      delta < 0,
    );
    renderPaletteResults();
  }

  function activatePaletteResult(result: PaletteResult | undefined): void {
    if (!result) return;
    const target = paletteTarget(result);
    closePalette();
    if (target.view === "issues") {
      ctx.shell.setView("issues");
      void ctx.issues.openIssueDetail(target.issueId, elements.navIssues);
      return;
    }
    if (target.view === "overview") {
      ctx.shell.setView("overview");
      return;
    }
    ctx.drawer.requestPendingTaskDetail(target.taskId);
    ctx.shell.setView("board");
    if (Number(ctx.state.board?.planId) === Number(target.planId)) {
      ctx.drawer.openPendingTaskDetail();
    } else {
      ctx.board.selectPlan(target.planId);
    }
  }

  function bind(): void {
    elements.paletteInput.addEventListener("input", schedulePaletteSearch);
    elements.paletteInput.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        movePaletteActive(event.key === "ArrowDown" ? 1 : -1);
      } else if (event.key === "Enter") {
        event.preventDefault();
        activatePaletteResult(paletteItems[paletteActive]);
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        closePalette();
      }
    });
    document.querySelectorAll("[data-close-palette]").forEach((closer) => {
      closer.addEventListener("click", closePalette);
    });
  }

  return {
    bind,
    openPalette,
    closePalette,
  };
}

export type Palette = ReturnType<typeof createPalette>;
