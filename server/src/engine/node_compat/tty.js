// node:tty — minimal subset. The sandbox has no controlling terminal, so
// isatty is always false and the stream classes exist mainly so code (and
// tests) can probe the prototype chain; they extend net.Socket like Node's.

import { Socket } from 'node:net';

export function isatty(_fd) {
    return false;
}

export class ReadStream extends Socket {
    constructor(_fd, options) {
        super(options);
        this.isRaw = false;
        this.isTTY = true;
    }

    setRawMode(mode) {
        this.isRaw = Boolean(mode);
        return this;
    }
}

export class WriteStream extends Socket {
    constructor(_fd) {
        super();
        this.columns = 80;
        this.rows = 24;
        this.isTTY = true;
    }

    hasColors(count) {
        if (typeof count === 'number') return count <= 16;
        return false;
    }

    getColorDepth() {
        return 1;
    }

    getWindowSize() {
        return [this.columns, this.rows];
    }

    clearLine(_dir, callback) {
        if (callback) callback();
        return true;
    }

    clearScreenDown(callback) {
        if (callback) callback();
        return true;
    }

    cursorTo(_x, _y, callback) {
        if (typeof _y === 'function') callback = _y;
        if (callback) callback();
        return true;
    }

    moveCursor(_dx, _dy, callback) {
        if (callback) callback();
        return true;
    }
}

export default { isatty, ReadStream, WriteStream };
