// node:stream/web — the WHATWG streams classes the runtime already
// provides as globals (see web_compat), re-exported under Node's module
// name. Classes the runtime doesn't implement export as undefined, so
// feature detection reads the same as on a Node build without them.

const g = globalThis;

export const ReadableStream = g.ReadableStream;
export const ReadableStreamDefaultReader = g.ReadableStreamDefaultReader;
export const ReadableStreamBYOBReader = g.ReadableStreamBYOBReader;
export const ReadableStreamBYOBRequest = g.ReadableStreamBYOBRequest;
export const ReadableByteStreamController = g.ReadableByteStreamController;
export const ReadableStreamDefaultController = g.ReadableStreamDefaultController;
export const TransformStream = g.TransformStream;
export const TransformStreamDefaultController = g.TransformStreamDefaultController;
export const WritableStream = g.WritableStream;
export const WritableStreamDefaultWriter = g.WritableStreamDefaultWriter;
export const WritableStreamDefaultController = g.WritableStreamDefaultController;
export const ByteLengthQueuingStrategy = g.ByteLengthQueuingStrategy;
export const CountQueuingStrategy = g.CountQueuingStrategy;
export const TextEncoderStream = g.TextEncoderStream;
export const TextDecoderStream = g.TextDecoderStream;
export const CompressionStream = g.CompressionStream;
export const DecompressionStream = g.DecompressionStream;

export default {
    ReadableStream,
    ReadableStreamDefaultReader,
    ReadableStreamBYOBReader,
    ReadableStreamBYOBRequest,
    ReadableByteStreamController,
    ReadableStreamDefaultController,
    TransformStream,
    TransformStreamDefaultController,
    WritableStream,
    WritableStreamDefaultWriter,
    WritableStreamDefaultController,
    ByteLengthQueuingStrategy,
    CountQueuingStrategy,
    TextEncoderStream,
    TextDecoderStream,
    CompressionStream,
    DecompressionStream,
};
