//! Modeled global functions, constructors and methods.
//!
//! These are the only calls of code outside the analyzed files that a
//! summary may treat as inert. Each entry says which arguments are callbacks
//! and when they run. The table assumes the globals are not replaced
//! (`docs/architecture/value-analysis.md`, §10) and that `console` output is not a
//! consistency-relevant effect.

/// When a builtin runs a callback argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Callback {
    /// During the call (`new Promise(executor)`, `arr.map(fn)`).
    Now,
    /// Later, outside the current sequence (`setTimeout(fn)`).
    Deferred,
}

/// What a modeled builtin does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Builtin {
    /// Argument index of a callback, and when it runs.
    pub callback: Option<(usize, Callback)>,
    /// The call may change its first argument (`Object.assign(target, …)`)
    /// or its receiver (`arr.push(x)`).
    pub mutates: bool,
    /// The call ends the process (`process.exit`): never inert.
    pub terminates: bool,
}

const fn pure() -> Builtin {
    Builtin {
        callback: None,
        mutates: false,
        terminates: false,
    }
}

const fn calls(index: usize, when: Callback) -> Builtin {
    Builtin {
        callback: Some((index, when)),
        mutates: false,
        terminates: false,
    }
}

const fn mutating() -> Builtin {
    Builtin {
        callback: None,
        mutates: true,
        terminates: false,
    }
}

/// A call of a global function (`setTimeout(...)`), the callee identifier
/// not being declared anywhere in the file.
pub(crate) fn global_function(name: &str) -> Option<Builtin> {
    Some(match name {
        "setTimeout"
        | "setInterval"
        | "setImmediate"
        | "queueMicrotask"
        | "requestAnimationFrame" => calls(0, Callback::Deferred),
        "clearTimeout" | "clearInterval" | "clearImmediate" | "parseInt" | "parseFloat"
        | "isNaN" | "isFinite" | "encodeURIComponent" | "decodeURIComponent" | "encodeURI"
        | "decodeURI" | "String" | "Number" | "Boolean" | "BigInt" | "Symbol"
        | "structuredClone" | "atob" | "btoa" => pure(),
        _ => return None,
    })
}

/// A call of a method of a global object (`Math.max(...)`), the root
/// identifier not being declared anywhere in the file.
pub(crate) fn global_method(object: &str, method: &str) -> Option<Builtin> {
    Some(match (object, method) {
        ("Math", _) | ("JSON", "parse" | "stringify") => pure(),
        (
            "Object",
            "keys"
            | "values"
            | "entries"
            | "freeze"
            | "isFrozen"
            | "fromEntries"
            | "getPrototypeOf"
            | "hasOwn"
            | "is"
            | "create"
            | "getOwnPropertyNames",
        ) => pure(),
        ("Object", "assign") => mutating(),
        ("Array", "isArray" | "of") => pure(),
        ("Array", "from") => calls(1, Callback::Now),
        (
            "Number",
            "isInteger" | "isFinite" | "isNaN" | "isSafeInteger" | "parseFloat" | "parseInt",
        ) => pure(),
        ("Date", "now" | "parse" | "UTC") => pure(),
        ("Promise", "resolve" | "reject" | "all" | "allSettled" | "race" | "any") => pure(),
        ("console", _) => pure(),
        ("crypto", "randomUUID" | "getRandomValues") => pure(),
        ("Buffer", "from" | "alloc" | "concat" | "isBuffer" | "byteLength") => pure(),
        ("performance", "now") => pure(),
        ("String", "raw" | "fromCharCode") => pure(),
        ("process", "exit" | "abort" | "kill") => Builtin {
            callback: None,
            mutates: false,
            terminates: true,
        },
        _ => return None,
    })
}

/// `new X(...)` of a global constructor.
pub(crate) fn global_constructor(name: &str) -> Option<Builtin> {
    Some(match name {
        "Promise" => calls(0, Callback::Now),
        "Error" | "TypeError" | "RangeError" | "SyntaxError" | "ReferenceError" | "EvalError"
        | "URIError" | "AggregateError" | "Map" | "Set" | "WeakMap" | "WeakSet" | "Date"
        | "URL" | "URLSearchParams" | "RegExp" | "Array" | "Object" | "Uint8Array"
        | "Int32Array" | "Float64Array" | "ArrayBuffer" | "TextEncoder" | "TextDecoder"
        | "AbortController" => pure(),
        _ => return None,
    })
}

/// A method of a value known to be an array.
pub(crate) fn array_method(method: &str) -> Option<Builtin> {
    Some(match method {
        "map" | "forEach" | "filter" | "find" | "findIndex" | "findLast" | "findLastIndex"
        | "some" | "every" | "flatMap" | "reduce" | "reduceRight" | "sort" | "toSorted" => {
            calls(0, Callback::Now)
        }
        "join" | "slice" | "concat" | "includes" | "indexOf" | "lastIndexOf" | "at" | "flat"
        | "keys" | "values" | "entries" | "toString" | "toReversed" | "with" => pure(),
        "push" | "pop" | "shift" | "unshift" | "splice" | "fill" | "copyWithin" | "reverse" => {
            mutating()
        }
        _ => return None,
    })
}

/// A method of a value known to be a string.
pub(crate) fn string_method(method: &str) -> Option<Builtin> {
    Some(match method {
        "replace" | "replaceAll" => Builtin {
            callback: Some((1, Callback::Now)),
            mutates: false,
            terminates: false,
        },
        "toLowerCase" | "toUpperCase" | "trim" | "trimStart" | "trimEnd" | "split" | "slice"
        | "substring" | "includes" | "startsWith" | "endsWith" | "padStart" | "padEnd"
        | "charAt" | "charCodeAt" | "indexOf" | "lastIndexOf" | "at" | "concat" | "repeat"
        | "normalize" | "localeCompare" | "match" | "matchAll" | "search" | "toString" => pure(),
        _ => return None,
    })
}
