export type StreamState = "closed" | "connecting" | "open" | "error";

type SocketEventType = "open" | "close" | "error" | "message";
type SocketListener = (event: { data?: unknown }) => void;

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
  onGap?(): void;
}

const outputWindowBytes = 512 * 1024;
// The one control frame the server sends, once, before a truncated replay.
const gapControl = `{"type":"gap"}`;

export class TerminalStreamClient {
  readonly #options: TerminalStreamClientOptions;
  #socket: WebSocketLike | null = null;
  #state: StreamState = "closed";
  #queue: Uint8Array[] = [];
  #bufferedBytes = 0;
  #writing = false;
  #generation = 0;
  #consumed = false;

  readonly #onOpen: SocketListener = () => {
    if (!this.#socket) return;
    this.#setState("open");
  };

  readonly #onClose: SocketListener = () => {
    if (!this.#socket) return;
    this.#detachSocket(false);
    this.#setState("closed");
  };

  readonly #onError: SocketListener = () => {
    if (!this.#socket) return;
    this.#fail();
  };

  readonly #onMessage: SocketListener = (event) => {
    if (this.#state === "open" && event.data === gapControl) {
      this.#options.onGap?.();
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
