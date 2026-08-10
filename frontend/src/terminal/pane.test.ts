import { describe, expect, it, vi } from "vitest";

import { TerminalStreamClient } from "./client";

type SocketEventType = "open" | "close" | "error" | "message";
type SocketListener = (event: { data?: unknown }) => void;

class FakeWebSocket {
  binaryType = "blob";
  readyState = 0;
  readonly sent: unknown[] = [];
  closeCalls = 0;
  sendError: Error | null = null;

  private readonly listeners = new Map<SocketEventType, Set<SocketListener>>();

  addEventListener(type: SocketEventType, listener: SocketListener) {
    let listeners = this.listeners.get(type);
    if (!listeners) {
      listeners = new Set();
      this.listeners.set(type, listeners);
    }
    listeners.add(listener);
  }

  removeEventListener(type: SocketEventType, listener: SocketListener) {
    this.listeners.get(type)?.delete(listener);
  }

  send(data: unknown) {
    if (this.sendError) throw this.sendError;
    this.sent.push(data);
  }

  close() {
    this.closeCalls += 1;
    this.readyState = 3;
  }

  open() {
    this.readyState = 1;
    this.dispatch("open");
  }

  remoteClose() {
    this.readyState = 3;
    this.dispatch("close");
  }

  fail() {
    this.dispatch("error");
  }

  beginClosing() {
    this.readyState = 2;
  }

  receive(data: unknown) {
    this.dispatch("message", { data });
  }

  listenerCount() {
    let count = 0;
    for (const listeners of this.listeners.values()) {
      count += listeners.size;
    }
    return count;
  }

  private dispatch(type: SocketEventType, event: { data?: unknown } = {}) {
    for (const listener of [...(this.listeners.get(type) ?? [])]) {
      listener(event);
    }
  }
}

describe("TerminalStreamClient", () => {
  it("moves through connecting, open, and closed states and requests ArrayBuffer frames", () => {
    const sockets: FakeWebSocket[] = [];
    const urls: string[] = [];
    const states: string[] = [];
    const client = new TerminalStreamClient({
      createWebSocket(url: string) {
        urls.push(url);
        const socket = new FakeWebSocket();
        sockets.push(socket);
        return socket;
      },
      writeOutput: vi.fn(),
      onStateChange(state: string) {
        states.push(state);
      },
    });

    expect(client.state).toBe("closed");
    client.connect("ws://127.0.0.1:49152/terminal/session?token=opaque");

    expect(client.state).toBe("connecting");
    expect(urls).toEqual(["ws://127.0.0.1:49152/terminal/session?token=opaque"]);
    expect(sockets[0].binaryType).toBe("arraybuffer");

    sockets[0].open();
    expect(client.state).toBe("open");

    sockets[0].remoteClose();
    expect(client.state).toBe("closed");
    expect(states).toEqual(["connecting", "open", "closed"]);
  });

  it("reports socket errors without a live Wails or DOM runtime", () => {
    const socket = new FakeWebSocket();
    const states: string[] = [];
    const client = new TerminalStreamClient({
      createWebSocket: () => socket,
      writeOutput: vi.fn(),
      onStateChange: (state: string) => states.push(state),
    });

    client.connect("ws://127.0.0.1/terminal/session");
    socket.fail();

    expect(client.state).toBe("error");
    expect(states).toEqual(["connecting", "error"]);
  });

  it("writes output sequentially and ACKs exact bytes only after each write callback", () => {
    const socket = new FakeWebSocket();
    const writes: Array<{ bytes: Uint8Array; done: () => void }> = [];
    const client = new TerminalStreamClient({
      createWebSocket: () => socket,
      writeOutput(bytes: Uint8Array, done: () => void) {
        writes.push({ bytes, done });
      },
      onStateChange: vi.fn(),
    });
    client.connect("ws://127.0.0.1/terminal/session");
    socket.open();

    socket.receive(new Uint8Array([1, 2, 3]).buffer);
    socket.receive(new Uint8Array([4, 5]).buffer);

    expect(writes).toHaveLength(1);
    expect([...writes[0].bytes]).toEqual([1, 2, 3]);
    expect(socket.sent).toEqual([]);

    writes[0].done();
    expect(socket.sent).toEqual([`{"type":"ack","bytes":3}`]);
    expect(writes).toHaveLength(2);
    expect([...writes[1].bytes]).toEqual([4, 5]);

    writes[1].done();
    expect(socket.sent).toEqual([
      `{"type":"ack","bytes":3}`,
      `{"type":"ack","bytes":2}`,
    ]);
  });

  it("reports only valid nonempty bounded output frames without exposing bytes", () => {
    const onOutput = vi.fn();
    const validSocket = new FakeWebSocket();
    const validClient = new TerminalStreamClient({
      createWebSocket: () => validSocket,
      writeOutput: vi.fn(),
      onStateChange: vi.fn(),
      onOutput,
    });
    validClient.connect("ws://127.0.0.1/terminal/session");
    validSocket.open();
    validSocket.receive(new Uint8Array([1, 2, 3]).buffer);
    expect(onOutput).toHaveBeenCalledOnce();
    expect(onOutput).toHaveBeenCalledWith(3);

    for (const invalid of [
      "not binary",
      new ArrayBuffer(0),
      new Uint8Array(512 * 1024 + 1).buffer,
    ]) {
      const socket = new FakeWebSocket();
      const invalidOutput = vi.fn();
      const client = new TerminalStreamClient({
        createWebSocket: () => socket,
        writeOutput: vi.fn(),
        onStateChange: vi.fn(),
        onOutput: invalidOutput,
      });
      client.connect("ws://127.0.0.1/terminal/session");
      socket.open();
      socket.receive(invalid);
      expect(invalidOutput).not.toHaveBeenCalled();
      expect(client.state).toBe("error");
    }
  });

  it("sends terminal input as binary only while the stream is open", () => {
    const socket = new FakeWebSocket();
    const client = new TerminalStreamClient({
      createWebSocket: () => socket,
      writeOutput: vi.fn(),
      onStateChange: vi.fn(),
    });
    const beforeOpen = new Uint8Array([1]);
    const whileOpen = new Uint8Array([2, 3]);
    const afterClose = new Uint8Array([4]);

    client.sendInput(beforeOpen);
    client.connect("ws://127.0.0.1/terminal/session");
    client.sendInput(beforeOpen);
    expect(socket.sent).toEqual([]);

    socket.open();
    client.sendInput(whileOpen);
    expect(socket.sent).toHaveLength(1);
    expect(socket.sent[0]).toBeInstanceOf(Uint8Array);
    expect([...(socket.sent[0] as Uint8Array)]).toEqual([2, 3]);

    client.close();
    client.sendInput(afterClose);
    expect(socket.sent).toHaveLength(1);
  });

  it("fails closed on nonbinary server output", () => {
    const socket = new FakeWebSocket();
    const writeOutput = vi.fn();
    const states: string[] = [];
    const client = new TerminalStreamClient({
      createWebSocket: () => socket,
      writeOutput,
      onStateChange: (state: string) => states.push(state),
    });
    client.connect("ws://127.0.0.1/terminal/session");
    socket.open();

    socket.receive(`{"type":"pty-output","data":"not-binary"}`);

    expect(client.state).toBe("error");
    expect(writeOutput).not.toHaveBeenCalled();
    expect(socket.closeCalls).toBe(1);
    expect(states).toEqual(["connecting", "open", "error"]);
  });

  it("caps queued plus in-progress output at the protocol window", () => {
    const socket = new FakeWebSocket();
    const writes: Array<{ bytes: Uint8Array; done: () => void }> = [];
    const states: string[] = [];
    const client = new TerminalStreamClient({
      createWebSocket: () => socket,
      writeOutput(bytes: Uint8Array, done: () => void) {
        writes.push({ bytes, done });
      },
      onStateChange: (state: string) => states.push(state),
    });
    client.connect("ws://127.0.0.1/terminal/session");
    socket.open();

    const chunk = new Uint8Array(64 * 1024).buffer;
    for (let index = 0; index < 8; index += 1) socket.receive(chunk.slice(0));
    expect(client.state).toBe("open");
    expect(writes).toHaveLength(1);

    socket.receive(chunk.slice(0));
    expect(client.state).toBe("error");
    expect(socket.closeCalls).toBe(1);
    expect(states.at(-1)).toBe("error");
  });

  it("fails closed when an ACK cannot be sent", () => {
    const socket = new FakeWebSocket();
    let writeDone: (() => void) | null = null;
    const client = new TerminalStreamClient({
      createWebSocket: () => socket,
      writeOutput(_bytes: Uint8Array, done: () => void) {
        writeDone = done;
      },
      onStateChange: vi.fn(),
    });
    client.connect("ws://127.0.0.1/terminal/session");
    socket.open();
    socket.receive(new Uint8Array([1, 2, 3]).buffer);
    socket.sendError = new Error("send failed");

    writeDone?.();
    expect(client.state).toBe("error");
    expect(socket.closeCalls).toBe(1);
  });

  it("does not ACK after the socket starts closing", () => {
    const socket = new FakeWebSocket();
    let writeDone: (() => void) | null = null;
    const client = new TerminalStreamClient({
      createWebSocket: () => socket,
      writeOutput(_bytes: Uint8Array, done: () => void) {
        writeDone = done;
      },
      onStateChange: vi.fn(),
    });
    client.connect("ws://127.0.0.1/terminal/session");
    socket.open();
    socket.receive(new Uint8Array([1]).buffer);
    socket.beginClosing();

    writeDone?.();
    expect(socket.sent).toEqual([]);
    expect(client.state).toBe("error");
  });

  it("tears down exactly once, removes listeners, and never ACKs or sends after close", () => {
    const socket = new FakeWebSocket();
    const writes: Array<{ bytes: Uint8Array; done: () => void }> = [];
    const states: string[] = [];
    const client = new TerminalStreamClient({
      createWebSocket: () => socket,
      writeOutput(bytes: Uint8Array, done: () => void) {
        writes.push({ bytes, done });
      },
      onStateChange: (state: string) => states.push(state),
    });
    client.connect("ws://127.0.0.1/terminal/session");
    socket.open();
    socket.receive(new Uint8Array([1, 2, 3, 4]).buffer);
    expect(writes).toHaveLength(1);
    expect(socket.listenerCount()).toBeGreaterThan(0);

    client.close();
    client.close();
    writes[0].done();
    client.sendInput(new Uint8Array([5]));
    socket.receive(new Uint8Array([6]).buffer);
    socket.open();

    expect(client.state).toBe("closed");
    expect(socket.closeCalls).toBe(1);
    expect(socket.listenerCount()).toBe(0);
    expect(socket.sent).toEqual([]);
    expect(writes).toHaveLength(1);
    expect(states).toEqual(["connecting", "open", "closed"]);
  });
});
