export type StreamState = "closed" | "connecting" | "open" | "error";

type SocketEventType = "open" | "close" | "error" | "message";
type SocketListener = (event: { data?: unknown; code?: number }) => void;

interface WebSocketLike {
  binaryType: string;
  readonly readyState: number;
  addEventListener(type: SocketEventType, listener: SocketListener): void;
  removeEventListener(type: SocketEventType, listener: SocketListener): void;
  send(data: string | Uint8Array): void;
  close(): void;
}

interface TerminalStreamClientOptions {
  createWebSocket(url: string): WebSocketLike;
  writeOutput(bytes: Uint8Array, done: () => void): void;
  onStateChange(state: StreamState): void;
  onOutput?(byteLength: number): void;
  /**
   * The replay was truncated. `sequence` is where it actually resumes — the
   * renderer's own count restarts there — or null from a server too old to say.
   */
  onGap?(sequence: number | null): void;
}

const outputWindowBytes = 512 * 1024;
// The one control frame the server sends, once, before a truncated replay,
// naming the sequence the replay resumes from.
const gapControl = /^\{"type":"gap"(?:,"sequence":(0|[1-9][0-9]{0,15}))?\}$/;
// The server closes with 1000 only once the session's output ended: the
// shell exited or its PTY closed. Nothing is left to re-claim.
const normalClosure = 1000;

function gapSequence(data: unknown): number | null | undefined {
  if (typeof data !== "string") return undefined;
  const match = gapControl.exec(data);
  if (!match) return undefined;
  if (match[1] === undefined) return null;
  const sequence = Number(match[1]);
  return Number.isSafeInteger(sequence) ? sequence : undefined;
}

export class TerminalStreamClient {
  readonly #options: TerminalStreamClientOptions;
  #socket: WebSocketLike | null = null;
  #state: StreamState = "closed";
  #queue: Uint8Array[] = [];
  #bufferedBytes = 0;
  #writing = false;
  #generation = 0;
  #consumed = false;
  #outputEnded = false;

  readonly #onOpen: SocketListener = () => {
    if (!this.#socket) return;
    this.#setState("open");
  };

  readonly #onClose: SocketListener = (event) => {
    if (!this.#socket) return;
    this.#outputEnded = event?.code === normalClosure;
    this.#detachSocket(false);
    this.#setState("closed");
  };

  readonly #onError: SocketListener = () => {
    if (!this.#socket) return;
    this.#fail();
  };

  readonly #onMessage: SocketListener = (event) => {
    const gap = this.#state === "open" ? gapSequence(event.data) : undefined;
    if (gap !== undefined) {
      this.#options.onGap?.(gap);
      return;
    }
    if (this.#state !== "open" || !(event.data instanceof ArrayBuffer)) {
      this.#fail();
      return;
    }
    const output = new Uint8Array(event.data);
    if (
      output.byteLength === 0 ||
      this.#bufferedBytes + output.byteLength > outputWindowBytes
    ) {
      this.#fail();
      return;
    }
    this.#options.onOutput?.(output.byteLength);
    this.#bufferedBytes += output.byteLength;
    this.#queue.push(output);
    this.#writeNext();
  };

  constructor(options: TerminalStreamClientOptions) {
    this.#options = options;
  }

  get state(): StreamState {
    return this.#state;
  }

  /** The server closed the stream because the session's output ended. */
  get outputEnded(): boolean {
    return this.#outputEnded;
  }

  connect(url: string): void {
    if (this.#consumed) {
      throw new Error("terminal stream authority is single-use");
    }
    if (this.#socket || this.#state === "connecting" || this.#state === "open") {
      throw new Error("terminal stream is already connected");
    }
    this.#consumed = true;
    const socket = this.#options.createWebSocket(url);
    this.#socket = socket;
    socket.binaryType = "arraybuffer";
    socket.addEventListener("open", this.#onOpen);
    socket.addEventListener("close", this.#onClose);
    socket.addEventListener("error", this.#onError);
    socket.addEventListener("message", this.#onMessage);
    this.#setState("connecting");
  }

  sendInput(input: Uint8Array): void {
    const socket = this.#socket;
    if (!socket || this.#state !== "open" || socket.readyState !== 1 || input.byteLength === 0) {
      return;
    }
    try {
      socket.send(input);
    } catch {
      this.#fail();
    }
  }

  close(): void {
    if (!this.#socket) {
      if (this.#state !== "closed") this.#setState("closed");
      return;
    }
    this.#detachSocket(true);
    this.#setState("closed");
  }

  #writeNext(): void {
    if (this.#writing || this.#state !== "open") return;
    const output = this.#queue.shift();
    if (!output) return;

    this.#writing = true;
    const generation = this.#generation;
    this.#options.writeOutput(output, () => {
      if (generation !== this.#generation) return;
      this.#writing = false;
      this.#bufferedBytes = Math.max(0, this.#bufferedBytes - output.byteLength);
      const socket = this.#socket;
      if (!socket || this.#state !== "open" || socket.readyState !== 1) {
        this.#fail();
        return;
      }
      try {
        socket.send(JSON.stringify({ type: "ack", bytes: output.byteLength }));
      } catch {
        this.#fail();
        return;
      }
      this.#writeNext();
    });
  }

  #detachSocket(close: boolean): void {
    const socket = this.#socket;
    if (!socket) return;
    this.#socket = null;
    this.#generation += 1;
    this.#queue = [];
    this.#bufferedBytes = 0;
    this.#writing = false;
    socket.removeEventListener("open", this.#onOpen);
    socket.removeEventListener("close", this.#onClose);
    socket.removeEventListener("error", this.#onError);
    socket.removeEventListener("message", this.#onMessage);
    if (close && socket.readyState !== 3) socket.close();
  }

  #fail(): void {
    this.#setState("error");
    this.#detachSocket(true);
  }

  #setState(state: StreamState): void {
    if (this.#state === state) return;
    this.#state = state;
    this.#options.onStateChange(state);
  }
}
