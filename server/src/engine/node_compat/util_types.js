// node:util/types — the same predicate object exposed as util.types.

import { types } from 'node:util';

export const {
    isDate,
    isRegExp,
    isNativeError,
    isPromise,
    isMap,
    isSet,
    isWeakMap,
    isWeakSet,
    isArrayBuffer,
    isSharedArrayBuffer,
    isAnyArrayBuffer,
    isArrayBufferView,
    isTypedArray,
    isDataView,
    isUint8Array,
    isAsyncFunction,
    isGeneratorFunction,
    isProxy,
    isBoxedPrimitive,
    isModuleNamespaceObject,
} = types;

export default types;
