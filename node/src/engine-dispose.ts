import { Engine } from "./server.js";

declare module "./server.js" {
  interface EngineLike {
    /** Shut down the engine and release this handle. Repeated calls are safe. */
    dispose(): void;
  }

  interface Engine {
    dispose(): void;
  }
}

const disposed = new WeakSet<Engine>();

Engine.prototype.dispose = function (this: Engine): void {
  if (disposed.has(this)) {
    return;
  }
  disposed.add(this);
  try {
    this.close();
  } finally {
    this.uniffiDestroy();
  }
};
