// self / navigator / performance — the remaining Minimum Common API
// globals that need no host support. Last file in the web_compat
// injection sequence: cleans up the internal hook object.
(function () {
    'use strict';
    if (typeof globalThis.performance === 'object' && globalThis.performance !== null &&
        typeof globalThis.navigator === 'object' && globalThis.navigator !== null) {
        delete globalThis.__webCompatInternal;
        return;
    }

    // ── self ────────────────────────────────────────────────────────────
    if (typeof globalThis.self === 'undefined') {
        Object.defineProperty(globalThis, 'self', {
            get: function () { return globalThis; },
            set: function (v) {
                Object.defineProperty(globalThis, 'self', {
                    value: v, writable: true, enumerable: true, configurable: true,
                });
            },
            enumerable: true,
            configurable: true,
        });
    }

    // ── navigator ───────────────────────────────────────────────────────
    var UA = typeof globalThis.__mcpV8UserAgent === 'string'
        ? globalThis.__mcpV8UserAgent
        : 'mcp-v8';
    delete globalThis.__mcpV8UserAgent;

    var navigatorBrand = new WeakMap();
    class Navigator {
        constructor() {
            if (navigatorBrand.has(Navigator)) {
                throw new TypeError('Illegal constructor');
            }
            navigatorBrand.set(this, true);
        }
        get userAgent() {
            if (!navigatorBrand.has(this)) throw new TypeError('Illegal invocation');
            return UA;
        }
        get language() { return 'en-US'; }
        get languages() { return Object.freeze(['en-US']); }
        get hardwareConcurrency() { return 1; }
    }
    Object.defineProperty(Navigator.prototype, Symbol.toStringTag, {
        value: 'Navigator', configurable: true,
    });
    globalThis.Navigator = Navigator;
    globalThis.navigator = new Navigator();
    navigatorBrand.set(Navigator, true); // further construction is illegal

    // ── performance ─────────────────────────────────────────────────────
    var timeOrigin = Date.now();
    var lastNow = 0;

    function nowFromOrigin() {
        var t = Date.now() - timeOrigin;
        if (t < lastNow) t = lastNow; // clamp: monotonic even if wall clock steps back
        lastNow = t;
        return t;
    }

    var entryData = new WeakMap();
    class PerformanceEntry {
        constructor() { throw new TypeError('Illegal constructor'); }
        get name() { return entryData.get(this).name; }
        get entryType() { return entryData.get(this).entryType; }
        get startTime() { return entryData.get(this).startTime; }
        get duration() { return entryData.get(this).duration; }
        toJSON() {
            var d = entryData.get(this);
            return { name: d.name, entryType: d.entryType, startTime: d.startTime, duration: d.duration };
        }
    }
    Object.defineProperty(PerformanceEntry.prototype, Symbol.toStringTag, {
        value: 'PerformanceEntry', configurable: true,
    });

    function makeEntry(Ctor, name, entryType, startTime, duration, detail) {
        var entry = Object.create(Ctor.prototype);
        entryData.set(entry, {
            name: name, entryType: entryType,
            startTime: startTime, duration: duration, detail: detail,
        });
        return entry;
    }

    class PerformanceMark extends PerformanceEntry {
        constructor(markName, markOptions) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'PerformanceMark': 1 argument required, but only 0 present.");
            }
            var opts = markOptions || {};
            var start = opts.startTime !== undefined ? Number(opts.startTime) : nowFromOrigin();
            if (start < 0) {
                throw new TypeError("'startTime' cannot be negative");
            }
            var entry = makeEntry(PerformanceMark, String(markName), 'mark', start, 0,
                opts.detail !== undefined ? globalThis.structuredClone(opts.detail) : null);
            return entry;
        }
        get detail() { return entryData.get(this).detail; }
    }
    Object.defineProperty(PerformanceMark.prototype, Symbol.toStringTag, {
        value: 'PerformanceMark', configurable: true,
    });
    globalThis.PerformanceMark = PerformanceMark;

    class PerformanceMeasure extends PerformanceEntry {
        get detail() { return entryData.get(this).detail; }
    }
    Object.defineProperty(PerformanceMeasure.prototype, Symbol.toStringTag, {
        value: 'PerformanceMeasure', configurable: true,
    });
    globalThis.PerformanceMeasure = PerformanceMeasure;

    var perfBrand = new WeakMap();
    var perfEntries = [];

    class Performance extends EventTarget {
        constructor() {
            if (perfBrand.has(Performance)) {
                throw new TypeError('Illegal constructor');
            }
            super();
            perfBrand.set(this, true);
        }
        get timeOrigin() { return timeOrigin; }
        now() { return nowFromOrigin(); }
        mark(markName, markOptions) {
            var entry = new PerformanceMark(markName, markOptions);
            perfEntries.push(entry);
            return entry;
        }
        measure(measureName, startOrMeasureOptions, endMark) {
            var start = 0, end = nowFromOrigin(), detail = null;
            function resolveMark(nameOrTime) {
                if (typeof nameOrTime === 'number') return nameOrTime;
                var name = String(nameOrTime);
                for (var i = perfEntries.length - 1; i >= 0; i--) {
                    if (perfEntries[i].entryType === 'mark' && perfEntries[i].name === name) {
                        return perfEntries[i].startTime;
                    }
                }
                throw new DOMException(
                    "The mark '" + name + "' does not exist.", 'SyntaxError');
            }
            if (typeof startOrMeasureOptions === 'object' && startOrMeasureOptions !== null) {
                var o = startOrMeasureOptions;
                if (o.start !== undefined) start = resolveMark(o.start);
                if (o.end !== undefined) end = resolveMark(o.end);
                if (o.duration !== undefined) {
                    if (o.start !== undefined) end = start + Number(o.duration);
                    else if (o.end !== undefined) start = end - Number(o.duration);
                }
                if (o.detail !== undefined) detail = globalThis.structuredClone(o.detail);
            } else if (startOrMeasureOptions !== undefined) {
                start = resolveMark(startOrMeasureOptions);
                if (endMark !== undefined) end = resolveMark(endMark);
            }
            var entry = makeEntry(PerformanceMeasure, String(measureName), 'measure',
                start, end - start, detail);
            perfEntries.push(entry);
            return entry;
        }
        getEntries() { return perfEntries.slice(); }
        getEntriesByType(type) {
            type = String(type);
            return perfEntries.filter(function (e) { return e.entryType === type; });
        }
        getEntriesByName(name, type) {
            name = String(name);
            return perfEntries.filter(function (e) {
                return e.name === name && (type === undefined || e.entryType === String(type));
            });
        }
        clearMarks(markName) {
            perfEntries = perfEntries.filter(function (e) {
                return e.entryType !== 'mark' ||
                    (markName !== undefined && e.name !== String(markName));
            });
        }
        clearMeasures(measureName) {
            perfEntries = perfEntries.filter(function (e) {
                return e.entryType !== 'measure' ||
                    (measureName !== undefined && e.name !== String(measureName));
            });
        }
        toJSON() { return { timeOrigin: timeOrigin }; }
    }
    Object.defineProperty(Performance.prototype, Symbol.toStringTag, {
        value: 'Performance', configurable: true,
    });
    globalThis.Performance = Performance;
    globalThis.performance = new Performance();
    perfBrand.set(Performance, true); // further construction is illegal

    delete globalThis.__webCompatInternal;
})();
