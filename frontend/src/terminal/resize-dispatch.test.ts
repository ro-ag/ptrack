import { describe, expect, it } from "vitest";

import { TerminalResizeDispatcher, type TerminalSize } from "./resize-dispatch";

class FakeClock {
  now = 1_000;
  nextTimer = 1;
  timers = new Map<number, { callback: () => void; delay: number }>();

  setTimer(callback: () => void, delay: number): number {
    const id = this.nextTimer++;
    this.timers.set(id, { callback, delay });
    return id;
  }

  clearTimer(id: number): void {
    this.timers.delete(id);
  }

  runNext(): number {
    const entry = this.timers.entries().next().value as
      [number, { callback: () => void; delay: number }] | undefined;
    if (!entry) throw new Error("no timer pending");
    const [id, timer] = entry;
    this.timers.delete(id);
    this.now += timer.delay;
    timer.callback();
    return timer.delay;
  }
}

describe("terminal resize dispatcher", () => {
  it("coalesces bursts, sends the trailing final size, and suppresses duplicates", () => {
    const clock = new FakeClock();
    const sent: TerminalSize[] = [];
    let dispatcher!: TerminalResizeDispatcher;
    dispatcher = new TerminalResizeDispatcher({
      now: () => clock.now,
      setTimer: (callback, delay) => clock.setTimer(callback, delay),
      clearTimer: (timer) => clock.clearTimer(timer),
      accepted: () => true,
      dispatch: (size) => {
        sent.push(size);
        if (sent.length === 1) {
          dispatcher.queue({ rows: 40, columns: 120 });
          dispatcher.queue({ rows: 41, columns: 121 });
        }
      },
    });

    dispatcher.queue({ rows: 24, columns: 80 });
    dispatcher.queue({ rows: 30, columns: 100 });
    expect(clock.runNext()).toBe(0);
    expect(sent).toEqual([{ rows: 30, columns: 100 }]);
    expect(clock.runNext()).toBe(100);
    expect(sent).toEqual([
      { rows: 30, columns: 100 },
      { rows: 41, columns: 121 },
    ]);
    dispatcher.queue({ rows: 41, columns: 121 });
    expect(clock.timers.size).toBe(0);
  });

  it("cancels stale/disposed work and permits a later retry after failure", () => {
    const clock = new FakeClock();
    const sent: TerminalSize[] = [];
    let accepted = false;
    const dispatcher = new TerminalResizeDispatcher({
      now: () => clock.now,
      setTimer: (callback, delay) => clock.setTimer(callback, delay),
      clearTimer: (timer) => clock.clearTimer(timer),
      accepted: () => accepted,
      dispatch: (size) => sent.push(size),
    });
    dispatcher.queue({ rows: 24, columns: 80 });
    clock.runNext();
    expect(sent).toEqual([]);

    accepted = true;
    dispatcher.queue({ rows: 24, columns: 80 });
    clock.runNext();
    expect(sent).toEqual([{ rows: 24, columns: 80 }]);
    dispatcher.invalidate({ rows: 24, columns: 80 });
    dispatcher.queue({ rows: 24, columns: 80 });
    clock.runNext();
    expect(sent).toEqual([
      { rows: 24, columns: 80 },
      { rows: 24, columns: 80 },
    ]);
    dispatcher.queue({ rows: 30, columns: 100 });
    dispatcher.dispose();
    expect(clock.timers.size).toBe(0);
  });

  it("cancels a pending backend resize when process-exit cleanup disposes it", () => {
    const clock = new FakeClock();
    const sent: TerminalSize[] = [];
    const dispatcher = new TerminalResizeDispatcher({
      now: () => clock.now,
      setTimer: (callback, delay) => clock.setTimer(callback, delay),
      clearTimer: (timer) => clock.clearTimer(timer),
      accepted: () => true,
      dispatch: (size) => sent.push(size),
    });

    dispatcher.queue({ rows: 24, columns: 80 });
    dispatcher.dispose();

    expect(clock.timers.size).toBe(0);
    expect(sent).toEqual([]);
  });
});
