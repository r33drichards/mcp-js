// Event / EventTarget / DOMException / AbortController / AbortSignal /
// reportError — the DOM plumbing layer of the Minimum Common Web Platform
// API. Pure JS so it lives entirely in the V8 heap and survives heap
// snapshot persistence. Guarded so re-injection into a restored heap that
// already has these globals is a no-op.
(function () {
    'use strict';
    if (typeof globalThis.EventTarget === 'function' &&
        typeof globalThis.AbortController === 'function') {
        return;
    }

    function defineName(fn, name) {
        Object.defineProperty(fn, 'name', { value: name, configurable: true });
    }

    // ── DOMException ────────────────────────────────────────────────────
    var LEGACY_CODES = {
        IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4,
        InvalidCharacterError: 5, NoModificationAllowedError: 7,
        NotFoundError: 8, NotSupportedError: 9, InUseAttributeError: 10,
        InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13,
        NamespaceError: 14, InvalidAccessError: 15, TypeMismatchError: 17,
        SecurityError: 18, NetworkError: 19, AbortError: 20,
        URLMismatchError: 21, QuotaExceededError: 22, TimeoutError: 23,
        InvalidNodeTypeError: 24, DataCloneError: 25,
    };
    var CONSTANTS = {
        INDEX_SIZE_ERR: 1, DOMSTRING_SIZE_ERR: 2, HIERARCHY_REQUEST_ERR: 3,
        WRONG_DOCUMENT_ERR: 4, INVALID_CHARACTER_ERR: 5, NO_DATA_ALLOWED_ERR: 6,
        NO_MODIFICATION_ALLOWED_ERR: 7, NOT_FOUND_ERR: 8, NOT_SUPPORTED_ERR: 9,
        INUSE_ATTRIBUTE_ERR: 10, INVALID_STATE_ERR: 11, SYNTAX_ERR: 12,
        INVALID_MODIFICATION_ERR: 13, NAMESPACE_ERR: 14, INVALID_ACCESS_ERR: 15,
        VALIDATION_ERR: 16, TYPE_MISMATCH_ERR: 17, SECURITY_ERR: 18,
        NETWORK_ERR: 19, ABORT_ERR: 20, URL_MISMATCH_ERR: 21,
        QUOTA_EXCEEDED_ERR: 22, TIMEOUT_ERR: 23, INVALID_NODE_TYPE_ERR: 24,
        DATA_CLONE_ERR: 25,
    };

    var domExceptionData = new WeakMap();

    class DOMException extends Error {
        constructor(message, name) {
            super();
            message = message === undefined ? '' : String(message);
            name = name === undefined ? 'Error' : String(name);
            domExceptionData.set(this, { message: message, name: name });
            if (Error.captureStackTrace) {
                Error.captureStackTrace(this, DOMException);
            }
        }
        get name() {
            var d = domExceptionData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.name;
        }
        get message() {
            var d = domExceptionData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.message;
        }
        get code() {
            var d = domExceptionData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return LEGACY_CODES[d.name] || 0;
        }
    }
    Object.defineProperty(DOMException.prototype, Symbol.toStringTag, {
        value: 'DOMException', configurable: true,
    });
    for (var constName in CONSTANTS) {
        var desc = { value: CONSTANTS[constName], enumerable: true };
        Object.defineProperty(DOMException, constName, desc);
        Object.defineProperty(DOMException.prototype, constName, desc);
    }
    globalThis.DOMException = DOMException;

    // QuotaExceededError graduated from a DOMException name to its own
    // interface (with quota/requested members) in the 2025 spec.
    var quotaData = new WeakMap();
    class QuotaExceededError extends DOMException {
        constructor(message, options) {
            super(message, 'QuotaExceededError');
            var quota = null, requested = null;
            if (options !== undefined && options !== null) {
                if (typeof options !== 'object' && typeof options !== 'function') {
                    throw new TypeError(
                        "Failed to construct 'QuotaExceededError': The provided value is not of type 'QuotaExceededErrorOptions'.");
                }
                if (options.quota !== undefined) {
                    quota = Number(options.quota);
                    if (!Number.isFinite(quota) || quota < 0) {
                        throw new RangeError(
                            "Failed to construct 'QuotaExceededError': quota must be a non-negative number.");
                    }
                }
                if (options.requested !== undefined) {
                    requested = Number(options.requested);
                    if (!Number.isFinite(requested) || requested < 0) {
                        throw new RangeError(
                            "Failed to construct 'QuotaExceededError': requested must be a non-negative number.");
                    }
                }
                if (quota !== null && requested !== null && requested < quota) {
                    throw new RangeError(
                        "Failed to construct 'QuotaExceededError': requested cannot be less than quota.");
                }
            }
            quotaData.set(this, { quota: quota, requested: requested });
        }
        get quota() {
            var d = quotaData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.quota;
        }
        get requested() {
            var d = quotaData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.requested;
        }
    }
    Object.defineProperty(QuotaExceededError.prototype, Symbol.toStringTag, {
        value: 'QuotaExceededError', configurable: true,
    });
    globalThis.QuotaExceededError = QuotaExceededError;

    // ── Event ───────────────────────────────────────────────────────────
    // Internal state lives in a WeakMap keyed by the event object.
    var eventData = new WeakMap();
    var timeOrigin = Date.now();

    function edata(ev) {
        var d = eventData.get(ev);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    function initEventData(ev, type, bubbles, cancelable, composed) {
        eventData.set(ev, {
            type: String(type),
            bubbles: !!bubbles,
            cancelable: !!cancelable,
            composed: !!composed,
            target: null,
            currentTarget: null,
            eventPhase: 0,
            canceled: false,
            stopPropagation: false,
            stopImmediatePropagation: false,
            isTrusted: false,
            dispatching: false,
            timeStamp: Date.now() - timeOrigin,
        });
    }

    function convertDict(init, what) {
        if (init === undefined || init === null) return {};
        if (typeof init !== 'object' && typeof init !== 'function') {
            throw new TypeError(
                "Failed to construct '" + what + "': The provided value is not of type '" +
                what + "Init'.");
        }
        return init;
    }

    class Event {
        constructor(type, eventInitDict) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'Event': 1 argument required, but only 0 present.");
            }
            var init = convertDict(eventInitDict, 'Event');
            initEventData(this, type, init.bubbles, init.cancelable, init.composed);
            // isTrusted is [LegacyUnforgeable]: an own, non-configurable
            // accessor on every instance.
            if (!Object.prototype.hasOwnProperty.call(this, 'isTrusted')) {
                var self_ = this;
                Object.defineProperty(this, 'isTrusted', {
                    get: function isTrusted() { return edata(self_).isTrusted; },
                    enumerable: true,
                    configurable: false,
                });
            }
        }
        get type() { return edata(this).type; }
        get target() { return edata(this).target; }
        get srcElement() { return edata(this).target; }
        get currentTarget() { return edata(this).currentTarget; }
        composedPath() {
            var d = edata(this);
            return d.currentTarget ? [d.currentTarget] : [];
        }
        get eventPhase() { return edata(this).eventPhase; }
        stopPropagation() { edata(this).stopPropagation = true; }
        get cancelBubble() { return edata(this).stopPropagation; }
        set cancelBubble(v) { if (v) edata(this).stopPropagation = true; }
        stopImmediatePropagation() {
            var d = edata(this);
            d.stopPropagation = true;
            d.stopImmediatePropagation = true;
        }
        get bubbles() { return edata(this).bubbles; }
        get cancelable() { return edata(this).cancelable; }
        get returnValue() { return !edata(this).canceled; }
        set returnValue(v) {
            var d = edata(this);
            if (!v && d.cancelable && !d.inPassiveListener) d.canceled = true;
        }
        preventDefault() {
            var d = edata(this);
            if (d.cancelable && !d.inPassiveListener) d.canceled = true;
        }
        get defaultPrevented() { return edata(this).canceled; }
        get composed() { return edata(this).composed; }
        get isTrusted() { return edata(this).isTrusted; }
        get timeStamp() { return edata(this).timeStamp; }
        initEvent(type, bubbles, cancelable) {
            var d = edata(this);
            if (d.dispatching) return;
            initEventData(this, type, bubbles, cancelable, d.composed);
        }
    }
    var PHASES = { NONE: 0, CAPTURING_PHASE: 1, AT_TARGET: 2, BUBBLING_PHASE: 3 };
    for (var phase in PHASES) {
        var pdesc = { value: PHASES[phase], enumerable: true };
        Object.defineProperty(Event, phase, pdesc);
        Object.defineProperty(Event.prototype, phase, pdesc);
    }
    Object.defineProperty(Event.prototype, Symbol.toStringTag, {
        value: 'Event', configurable: true,
    });
    globalThis.Event = Event;

    // ── Event subclasses ────────────────────────────────────────────────
    function subclassData(map) {
        return function (ev) {
            var d = map.get(ev);
            if (!d) throw new TypeError('Illegal invocation');
            return d;
        };
    }

    var customEventData = new WeakMap();
    var cedata = subclassData(customEventData);
    class CustomEvent extends Event {
        constructor(type, eventInitDict) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'CustomEvent': 1 argument required, but only 0 present.");
            }
            super(type, eventInitDict);
            var init = convertDict(eventInitDict, 'CustomEvent');
            customEventData.set(this, {
                detail: init.detail !== undefined ? init.detail : null,
            });
        }
        get detail() { return cedata(this).detail; }
        initCustomEvent(type, bubbles, cancelable, detail) {
            this.initEvent(type, bubbles, cancelable);
            if (arguments.length > 3) cedata(this).detail = detail;
        }
    }
    Object.defineProperty(CustomEvent.prototype, Symbol.toStringTag, {
        value: 'CustomEvent', configurable: true,
    });
    globalThis.CustomEvent = CustomEvent;

    var errorEventData = new WeakMap();
    var eedata = subclassData(errorEventData);
    class ErrorEvent extends Event {
        constructor(type, eventInitDict) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'ErrorEvent': 1 argument required, but only 0 present.");
            }
            super(type, eventInitDict);
            var init = convertDict(eventInitDict, 'ErrorEvent');
            errorEventData.set(this, {
                message: init.message !== undefined ? String(init.message) : '',
                filename: init.filename !== undefined ? String(init.filename) : '',
                lineno: init.lineno !== undefined ? (Number(init.lineno) >>> 0) : 0,
                colno: init.colno !== undefined ? (Number(init.colno) >>> 0) : 0,
                error: init.error,
            });
        }
        get message() { return eedata(this).message; }
        get filename() { return eedata(this).filename; }
        get lineno() { return eedata(this).lineno; }
        get colno() { return eedata(this).colno; }
        get error() { return eedata(this).error; }
    }
    Object.defineProperty(ErrorEvent.prototype, Symbol.toStringTag, {
        value: 'ErrorEvent', configurable: true,
    });
    globalThis.ErrorEvent = ErrorEvent;

    var prEventData = new WeakMap();
    var prdata = subclassData(prEventData);
    class PromiseRejectionEvent extends Event {
        constructor(type, eventInitDict) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to construct 'PromiseRejectionEvent': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            var init = convertDict(eventInitDict, 'PromiseRejectionEvent');
            if (!('promise' in init)) {
                throw new TypeError(
                    "Failed to construct 'PromiseRejectionEvent': required member promise is undefined.");
            }
            super(type, eventInitDict);
            prEventData.set(this, { promise: init.promise, reason: init.reason });
        }
        get promise() { return prdata(this).promise; }
        get reason() { return prdata(this).reason; }
    }
    Object.defineProperty(PromiseRejectionEvent.prototype, Symbol.toStringTag, {
        value: 'PromiseRejectionEvent', configurable: true,
    });
    globalThis.PromiseRejectionEvent = PromiseRejectionEvent;

    var messageEventData = new WeakMap();
    var medata = subclassData(messageEventData);
    class MessageEvent extends Event {
        constructor(type, eventInitDict) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'MessageEvent': 1 argument required, but only 0 present.");
            }
            super(type, eventInitDict);
            var init = convertDict(eventInitDict, 'MessageEvent');
            messageEventData.set(this, {
                data: init.data !== undefined ? init.data : null,
                origin: init.origin !== undefined ? String(init.origin) : '',
                lastEventId: init.lastEventId !== undefined ? String(init.lastEventId) : '',
                source: init.source !== undefined ? init.source : null,
                ports: Object.freeze(init.ports ? Array.from(init.ports) : []),
            });
        }
        get data() { return medata(this).data; }
        get origin() { return medata(this).origin; }
        get lastEventId() { return medata(this).lastEventId; }
        get source() { return medata(this).source; }
        get ports() { return medata(this).ports; }
    }
    Object.defineProperty(MessageEvent.prototype, Symbol.toStringTag, {
        value: 'MessageEvent', configurable: true,
    });
    globalThis.MessageEvent = MessageEvent;

    var closeEventData = new WeakMap();
    var cldata = subclassData(closeEventData);
    class CloseEvent extends Event {
        constructor(type, eventInitDict) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'CloseEvent': 1 argument required, but only 0 present.");
            }
            super(type, eventInitDict);
            var init = convertDict(eventInitDict, 'CloseEvent');
            closeEventData.set(this, {
                wasClean: !!init.wasClean,
                code: init.code !== undefined ? (Number(init.code) & 0xffff) : 0,
                reason: init.reason !== undefined ? String(init.reason) : '',
            });
        }
        get wasClean() { return cldata(this).wasClean; }
        get code() { return cldata(this).code; }
        get reason() { return cldata(this).reason; }
    }
    Object.defineProperty(CloseEvent.prototype, Symbol.toStringTag, {
        value: 'CloseEvent', configurable: true,
    });
    globalThis.CloseEvent = CloseEvent;

    // ── EventTarget ─────────────────────────────────────────────────────
    // Listener lists are stored in a WeakMap keyed by the receiver, so the
    // prototype methods also work when borrowed by globalThis below.
    var targetData = new WeakMap();

    function listeners(target) {
        var d = targetData.get(target);
        if (!d) {
            d = new Map();
            targetData.set(target, d);
        }
        return d;
    }

    function normalizeOptions(options) {
        if (typeof options === 'boolean') return { capture: options };
        if (options === undefined || options === null) return { capture: false };
        if (typeof options !== 'object' && typeof options !== 'function') {
            return { capture: !!options };
        }
        return {
            capture: !!options.capture,
            once: !!options.once,
            passive: !!options.passive,
            signal: options.signal,
        };
    }

    // removeEventListener only understands capture (the spec's
    // EventListenerOptions dictionary) — reading `passive` here would be
    // observable through getters.
    function normalizeRemoveOptions(options) {
        if (typeof options === 'boolean') return { capture: options };
        if (options === undefined || options === null) return { capture: false };
        if (typeof options !== 'object' && typeof options !== 'function') {
            return { capture: !!options };
        }
        return { capture: !!options.capture };
    }

    function addListener(target, type, callback, options) {
        var opts = normalizeOptions(options);
        if (opts.signal !== undefined) {
            if (!(opts.signal instanceof AbortSignal)) {
                throw new TypeError(
                    "Failed to execute 'addEventListener': member signal is not of type AbortSignal.");
            }
            if (opts.signal.aborted) return;
        }
        if (callback === null || callback === undefined) return;
        if (typeof callback !== 'function' &&
            (typeof callback !== 'object' || typeof callback.handleEvent === 'undefined')) {
            // Accept objects (handleEvent looked up at dispatch time) and
            // functions; primitives throw per WebIDL.
            if (typeof callback !== 'object') {
                throw new TypeError(
                    "Failed to execute 'addEventListener': parameter 2 is not of type 'EventListener'.");
            }
        }
        type = String(type);
        var map = listeners(target);
        var list = map.get(type);
        if (!list) {
            list = [];
            map.set(type, list);
        }
        for (var i = 0; i < list.length; i++) {
            if (list[i].callback === callback && list[i].capture === opts.capture) return;
        }
        var entry = {
            callback: callback,
            capture: opts.capture,
            once: !!opts.once,
            passive: !!opts.passive,
            removed: false,
        };
        list.push(entry);
        if (opts.signal !== undefined) {
            addListener(opts.signal, 'abort', function () {
                removeEntry(target, type, entry);
            }, {});
        }
    }

    function removeEntry(target, type, entry) {
        entry.removed = true;
        var list = listeners(target).get(type);
        if (!list) return;
        var idx = list.indexOf(entry);
        if (idx !== -1) list.splice(idx, 1);
    }

    function removeListener(target, type, callback, options) {
        var opts = normalizeRemoveOptions(options);
        type = String(type);
        var list = listeners(target).get(type);
        if (!list) return;
        for (var i = 0; i < list.length; i++) {
            if (list[i].callback === callback && list[i].capture === opts.capture) {
                list[i].removed = true;
                list.splice(i, 1);
                return;
            }
        }
    }

    function dispatch(target, event) {
        var d = eventData.get(event);
        if (!d) {
            throw new TypeError(
                "Failed to execute 'dispatchEvent': parameter 1 is not of type 'Event'.");
        }
        if (d.dispatching) {
            throw new DOMException(
                "Failed to execute 'dispatchEvent': The event is already being dispatched.",
                'InvalidStateError');
        }
        d.isTrusted = false;
        return dispatchInternal(target, event);
    }

    // Used both by dispatchEvent (isTrusted forced false above) and by
    // runtime-generated events (isTrusted preset by the caller).
    function dispatchInternal(target, event) {
        var d = eventData.get(event);
        d.dispatching = true;
        d.target = target;
        d.currentTarget = target;
        d.eventPhase = 2; // AT_TARGET
        var list = listeners(target).get(d.type);
        if (list) {
            var snapshot = list.slice();
            for (var i = 0; i < snapshot.length; i++) {
                var entry = snapshot[i];
                if (entry.removed) continue;
                if (entry.once) removeEntry(target, d.type, entry);
                if (entry.passive) d.inPassiveListener = true;
                try {
                    if (typeof entry.callback === 'function') {
                        entry.callback.call(target, event);
                    } else if (entry.callback &&
                               typeof entry.callback.handleEvent === 'function') {
                        entry.callback.handleEvent(event);
                    }
                } catch (err) {
                    reportThrown(err);
                } finally {
                    d.inPassiveListener = false;
                }
                if (d.stopImmediatePropagation) break;
            }
        }
        d.eventPhase = 0; // NONE
        d.currentTarget = null;
        d.dispatching = false;
        d.stopPropagation = false;
        d.stopImmediatePropagation = false;
        return !d.canceled;
    }

    class EventTarget {
        constructor() {
            targetData.set(this, new Map());
        }
        addEventListener(type, callback, options) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'addEventListener': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            addListener(this, type, callback, options);
        }
        removeEventListener(type, callback, options) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'removeEventListener': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            removeListener(this, type, callback, options);
        }
        dispatchEvent(event) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'dispatchEvent': 1 argument required, but only 0 present.");
            }
            return dispatch(this, event);
        }
    }
    Object.defineProperty(EventTarget.prototype, Symbol.toStringTag, {
        value: 'EventTarget', configurable: true,
    });
    globalThis.EventTarget = EventTarget;

    // Event handler IDL attributes (onabort, onmessage, ...) — shared with
    // the other web_compat files via __webCompatInternal below.
    var handlerData = new WeakMap();
    function defineEventHandler(proto, name) {
        var type = name.slice(2);
        Object.defineProperty(proto, name, {
            enumerable: true,
            configurable: true,
            get: function () {
                var map = handlerData.get(this);
                var rec = map && map.get(name);
                return rec ? rec.handler : null;
            },
            set: function (value) {
                var map = handlerData.get(this);
                if (!map) {
                    map = new Map();
                    handlerData.set(this, map);
                }
                var old = map.get(name);
                if (old) {
                    removeListener(this, type, old.wrapper, {});
                    map.delete(name);
                }
                var isObject = typeof value === 'function' ||
                    (typeof value === 'object' && value !== null);
                if (isObject) {
                    var wrapper = function (ev) {
                        if (typeof value === 'function') return value.call(this, ev);
                    };
                    map.set(name, { handler: value, wrapper: wrapper });
                    addListener(this, type, wrapper, {});
                }
            },
        });
    }

    // ── AbortSignal / AbortController ───────────────────────────────────
    var signalData = new WeakMap();
    var allowConstruct = false;

    function sdata(signal) {
        var d = signalData.get(signal);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    function createSignal() {
        allowConstruct = true;
        try {
            return new AbortSignal();
        } finally {
            allowConstruct = false;
        }
    }

    function signalAbort(signal, reason) {
        var d = sdata(signal);
        if (d.aborted) return;
        d.aborted = true;
        d.reason = reason !== undefined
            ? reason
            : new DOMException('signal is aborted without reason', 'AbortError');
        // Per spec, every dependent signal's state flips before any abort
        // event fires; events then fire source-first, dependents in
        // registration order.
        var toFire = [signal];
        var dependents = d.dependents;
        void 0;
        d.dependents = [];
        for (var i = 0; i < dependents.length; i++) {
            var dd = sdata(dependents[i]);
            if (!dd.aborted) {
                dd.aborted = true;
                dd.reason = d.reason;
                toFire.push(dependents[i]);
            }
        }
        for (var j = 0; j < toFire.length; j++) {
            var ev = new Event('abort');
            eventData.get(ev).isTrusted = true;
            dispatchInternal(toFire[j], ev);
        }
    }

    class AbortSignal extends EventTarget {
        constructor() {
            if (!allowConstruct) throw new TypeError('Illegal constructor');
            super();
            signalData.set(this, { aborted: false, reason: undefined, dependents: [] });
        }
        static abort(reason) {
            var signal = createSignal();
            var d = sdata(signal);
            d.aborted = true;
            d.reason = reason !== undefined
                ? reason
                : new DOMException('signal is aborted without reason', 'AbortError');
            return signal;
        }
        static timeout(milliseconds) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'timeout': 1 argument required, but only 0 present.");
            }
            var ms = Number(milliseconds);
            if (!Number.isFinite(ms) || ms < 0) {
                throw new TypeError(
                    "Failed to execute 'timeout' on 'AbortSignal': Value is outside the 'unsigned long long' value range.");
            }
            var signal = createSignal();
            setTimeout(function () {
                signalAbort(signal, new DOMException('signal timed out', 'TimeoutError'));
            }, ms);
            return signal;
        }
        static any(signals) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'any': 1 argument required, but only 0 present.");
            }
            var list = Array.from(signals);
            for (var i = 0; i < list.length; i++) {
                if (!(list[i] instanceof AbortSignal)) {
                    throw new TypeError(
                        "Failed to execute 'any' on 'AbortSignal': Failed to convert value to 'AbortSignal'.");
                }
            }
            var result = createSignal();
            var rd = sdata(result);
            rd.dependent = true;
            rd.sources = [];
            for (var j = 0; j < list.length; j++) {
                if (list[j].aborted) {
                    rd.aborted = true;
                    rd.reason = sdata(list[j]).reason;
                    rd.sources = [];
                    return result;
                }
            }
            // Flatten: a dependent signal contributes its source signals, so
            // dependents always hang off originating (non-composite) signals.
            for (var k = 0; k < list.length; k++) {
                var sd = sdata(list[k]);
                var sources = sd.dependent ? sd.sources : [list[k]];
                for (var l = 0; l < sources.length; l++) {
                    if (rd.sources.indexOf(sources[l]) === -1) {
                        rd.sources.push(sources[l]);
                        sdata(sources[l]).dependents.push(result);
                    }
                }
            }
            return result;
        }
        get aborted() { return sdata(this).aborted; }
        get reason() { return sdata(this).reason; }
        throwIfAborted() {
            var d = sdata(this);
            if (d.aborted) throw d.reason;
        }
    }
    Object.defineProperty(AbortSignal.prototype, Symbol.toStringTag, {
        value: 'AbortSignal', configurable: true,
    });
    defineEventHandler(AbortSignal.prototype, 'onabort');
    globalThis.AbortSignal = AbortSignal;

    var controllerData = new WeakMap();
    class AbortController {
        constructor() {
            controllerData.set(this, createSignal());
        }
        get signal() {
            var s = controllerData.get(this);
            if (!s) throw new TypeError('Illegal invocation');
            return s;
        }
        abort(reason) {
            var s = controllerData.get(this);
            if (!s) throw new TypeError('Illegal invocation');
            signalAbort(s, reason);
        }
    }
    Object.defineProperty(AbortController.prototype, Symbol.toStringTag, {
        value: 'AbortController', configurable: true,
    });
    globalThis.AbortController = AbortController;

    // ── reportError + a global event target ─────────────────────────────
    // globalThis borrows the EventTarget machinery (listener storage is
    // WeakMap-keyed, so any receiver works).
    globalThis.addEventListener = EventTarget.prototype.addEventListener.bind(globalThis);
    globalThis.removeEventListener = EventTarget.prototype.removeEventListener.bind(globalThis);
    globalThis.dispatchEvent = EventTarget.prototype.dispatchEvent.bind(globalThis);
    defineName(globalThis.addEventListener, 'addEventListener');
    defineName(globalThis.removeEventListener, 'removeEventListener');
    defineName(globalThis.dispatchEvent, 'dispatchEvent');

    function reportThrown(err) {
        var message = 'Uncaught';
        try {
            message = 'Uncaught ' + (err instanceof Error ? (err.name + ': ' + err.message) : String(err));
        } catch (_) { /* String() itself can throw */ }
        var ev = new ErrorEvent('error', {
            message: message,
            error: err,
            cancelable: true,
        });
        var ok = false;
        try {
            ok = !dispatchInternal(globalThis, ev);
        } catch (_) { /* never let error reporting throw */ }
        if (!ok && typeof console !== 'undefined' && console.error) {
            console.error(message);
        }
    }

    globalThis.reportError = function reportError(e) {
        if (arguments.length < 1) {
            throw new TypeError(
                "Failed to execute 'reportError': 1 argument required, but only 0 present.");
        }
        reportThrown(e);
    };

    // Internal hooks for the other web_compat files (deleted by the last
    // file in the injection sequence).
    globalThis.__webCompatInternal = {
        dispatchInternal: dispatchInternal,
        defineEventHandler: defineEventHandler,
        initEventData: eventData,
        reportThrown: reportThrown,
    };
})();
