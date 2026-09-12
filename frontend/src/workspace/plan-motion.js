import { animate } from "motion/mini";
import { reducedMotionActive } from "../settings/preferences";

// Keep motion on the title so the row's hit target and actions never move.
export function bindPlanMotion(row, title) {
  let animation;
  let hovered = false;
  const move = (offset) => {
    animation?.stop();
    const reduced = reducedMotionActive(
      document.documentElement.dataset.reducedMotion || "system",
      window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    );
    if (reduced) {
      title.style.transform = "none";
      return;
    }
    animation = animate(title, { transform: `translateX(${offset}px)` }, {
      duration: 0.16,
      ease: [0.2, 0.8, 0.2, 1],
    });
  };
  row.addEventListener("pointerenter", (event) => {
    if (event.pointerType === "touch") return;
    hovered = true;
    move(4);
  });
  row.addEventListener("pointerleave", () => {
    hovered = false;
    move(0);
  });
  row.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || event.target.closest("button")) return;
    move(2);
  });
  row.addEventListener("pointerup", () => move(hovered ? 4 : 0));
  row.addEventListener("pointercancel", () => {
    hovered = false;
    move(0);
  });
}
