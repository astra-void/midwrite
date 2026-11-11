//! Package contracts.
//!
//! A call into a package is not analyzed (no package code is read or run).
//! Every package call falls in one of three categories:
//!
//! * **verified built-in contract** — a narrowly scoped entry of
//!   [`CONTRACTS`], written from the package's documented API and its
//!   published source at a pinned version: whether it is async, what it
//!   returns, how it fails, and that it has no database or external effect.
//!   A contract never invents a value: a result the model cannot represent
//!   (a salted hash, a validation outcome) stays unknown, and the call stays
//!   an `Op::Unknown` that names the contract; what the contract adds is the
//!   precise reason and the certainty that nothing else happens.
//! * **declared external contract** — a callee the project's
//!   `midwrite-contracts.json` declares (`externals`: an effect on a
//!   channel); lowered as an external effect by the extractor.
//! * **unknown package behavior** — everything else: `Op::Unknown`
//!   ("call into an unanalyzed package").
//!
//! No package is marked pure by its name alone: an entry matches a package
//! specifier and an export (or a method of a value that provably comes from
//! that package), nothing broader. Higher-order contracts (React `cache`)
//! decide whether and when a wrapped callback runs; they are applied by the
//! extractor with that invocation evidence (`REACT_CACHE_CONTRACT`).

/// What the model may do with a call covered by a contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modeled {
    /// The result is not representable: the call stays unknown (with the
    /// contract named).
    ResultNotRepresentable,
    /// The result is a fresh random value: modeled as a generated
    /// value (equal only to itself; the `generated-values-distinct`
    /// assumption), with no other effect.
    FreshValue,
}

/// A verified built-in contract for one package export or method.
#[derive(Clone, Copy, Debug)]
pub struct Contract {
    /// Package specifiers the contract applies to (exact match).
    pub packages: &'static [&'static str],
    /// The export (`hash`), or the method of a value of the package
    /// (`safeParse` of a schema).
    pub name: &'static str,
    /// The call is a method of a value created by the package (a zod
    /// schema), not an export.
    pub method_of_value: bool,
    pub is_async: bool,
    pub returns: &'static str,
    pub errors: &'static str,
    pub effects: &'static str,
    /// Where the contract was read.
    pub source: &'static str,
    pub modeled: Modeled,
}

const BCRYPT: &[&str] = &["bcryptjs", "bcrypt"];
const ZOD: &[&str] = &["zod", "zod/v4", "zod/v3"];

/// Whether `source` is the zod package.
pub fn is_zod(source: &str) -> bool {
    ZOD.contains(&source)
}
const NANOID: &[&str] = &["nanoid"];
const NODE_CRYPTO: &[&str] = &["crypto", "node:crypto"];
const NANOID_SOURCE: &str = "nanoid 3.x-5.x documented API: `nanoid(size = 21)` returns `size` random characters of the URL-safe alphabet (crypto.getRandomValues)";
const NODE_CRYPTO_SOURCE: &str =
    "Node.js `crypto.randomUUID()` documentation: a random RFC 4122 version 4 UUID string";
const BCRYPT_SOURCE: &str = "bcryptjs 2.x/3.x and bcrypt 5.x documented API (hash/compare/genSalt)";
const ZOD_SOURCE: &str = "zod 3.x/4.x documented ZodType API";

/// The verified built-in contracts.
pub const CONTRACTS: &[Contract] = &[
    Contract {
        packages: NANOID,
        name: "nanoid",
        method_of_value: false,
        is_async: false,
        returns: "a fresh random string (21 URL-safe characters unless a size is given)",
        errors: "throws on a non-numeric or negative size",
        effects: "none outside the process (reads randomness)",
        source: NANOID_SOURCE,
        modeled: Modeled::FreshValue,
    },
    Contract {
        packages: NODE_CRYPTO,
        name: "randomUUID",
        method_of_value: false,
        is_async: false,
        returns: "a fresh random version-4 UUID string",
        errors: "none for the default options",
        effects: "none outside the process (reads randomness)",
        source: NODE_CRYPTO_SOURCE,
        modeled: Modeled::FreshValue,
    },
    Contract {
        packages: BCRYPT,
        name: "hash",
        method_of_value: false,
        is_async: true,
        returns: "a salted hash string (a fresh value: the salt is random)",
        errors: "rejects on invalid arguments (a non-string input, an invalid salt or round count)",
        effects: "none outside the process (CPU only; no database, network or file access)",
        source: BCRYPT_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
    Contract {
        packages: BCRYPT,
        name: "compare",
        method_of_value: false,
        is_async: true,
        returns: "whether the string hashes to the given hash (depends on the hash value)",
        errors: "rejects on invalid arguments",
        effects: "none outside the process (CPU only)",
        source: BCRYPT_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
    Contract {
        packages: BCRYPT,
        name: "genSalt",
        method_of_value: false,
        is_async: true,
        returns: "a random salt string (a fresh value)",
        errors: "rejects on an invalid round count",
        effects: "none outside the process (reads randomness)",
        source: BCRYPT_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
    Contract {
        packages: BCRYPT,
        name: "hashSync",
        method_of_value: false,
        is_async: false,
        returns: "a salted hash string (a fresh value)",
        errors: "throws on invalid arguments",
        effects: "none outside the process (CPU only)",
        source: BCRYPT_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
    Contract {
        packages: BCRYPT,
        name: "compareSync",
        method_of_value: false,
        is_async: false,
        returns: "whether the string hashes to the given hash",
        errors: "throws on invalid arguments",
        effects: "none outside the process (CPU only)",
        source: BCRYPT_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
    Contract {
        packages: ZOD,
        name: "parse",
        method_of_value: true,
        is_async: false,
        returns: "the parsed output (the input as transformed by the schema: unknown keys stripped, defaults, coercions and transforms applied)",
        errors: "throws a ZodError when the input does not satisfy the schema (validation logic is not run by the model)",
        effects: "none outside the process unless the schema's own refinements or transforms have effects (not analyzed)",
        source: ZOD_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
    Contract {
        packages: ZOD,
        name: "safeParse",
        method_of_value: true,
        is_async: false,
        returns: "`{ success: true, data }` or `{ success: false, error }` (which one depends on validation logic the model does not run)",
        errors: "does not throw for invalid input",
        effects: "none outside the process unless the schema's own refinements or transforms have effects (not analyzed)",
        source: ZOD_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
    Contract {
        packages: ZOD,
        name: "parseAsync",
        method_of_value: true,
        is_async: true,
        returns: "the parsed output",
        errors: "rejects with a ZodError when the input does not satisfy the schema",
        effects: "none outside the process unless the schema's async refinements have effects (not analyzed)",
        source: ZOD_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
    Contract {
        packages: ZOD,
        name: "safeParseAsync",
        method_of_value: true,
        is_async: true,
        returns: "`{ success, data | error }`",
        errors: "does not reject for invalid input",
        effects: "none outside the process unless the schema's async refinements have effects (not analyzed)",
        source: ZOD_SOURCE,
        modeled: Modeled::ResultNotRepresentable,
    },
];

/// The React `cache` higher-order contract (react 19 `cache`, Server
/// Components): `cache(fn)` returns a function that, within one server
/// request, runs `fn` on the first call with given arguments and returns
/// that call's result (memoized) on later calls with the same arguments.
pub const REACT_CACHE_CONTRACT: &str = "verified built-in contract: react `cache(fn)` runs `fn` on the first call with given arguments within a server request and returns the memoized result on later calls";

/// Whether `imported` of `specifier` is React's `cache`.
pub fn is_react_cache(specifier: &str, imported: &str) -> bool {
    specifier == "react" && imported == "cache"
}

/// The contract of an export of a package (`import { hash } from
/// "bcryptjs"`, or `bcrypt.hash` of a default or namespace import).
pub fn export_contract(specifier: &str, name: &str) -> Option<&'static Contract> {
    CONTRACTS
        .iter()
        .find(|c| !c.method_of_value && c.name == name && c.packages.contains(&specifier))
}

/// The contract of a method of a value created by a package (a zod schema).
pub fn method_contract(specifier: &str, method: &str) -> Option<&'static Contract> {
    CONTRACTS
        .iter()
        .find(|c| c.method_of_value && c.name == method && c.packages.contains(&specifier))
}

/// The unknown-operation detail of a call covered by a contract.
pub fn describe(c: &Contract, written: &str, specifier: &str) -> String {
    let modeled = match c.modeled {
        Modeled::ResultNotRepresentable => "its result is not representable in the model",
        Modeled::FreshValue => "its result is modeled as a generated value",
    };
    format!(
        "package call with a verified built-in contract: `{written}` of `{specifier}` ({}; returns {}; {}; effects: {}; source: {}): {modeled}",
        if c.is_async { "async" } else { "synchronous" },
        c.returns,
        c.errors,
        c.effects,
        c.source
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_match_exact_packages_and_names_only() {
        assert!(export_contract("bcryptjs", "hash").is_some());
        assert!(export_contract("bcrypt", "compare").is_some());
        // Not by name alone: another package, another export, a method.
        assert!(export_contract("bcrypt-ts", "hash").is_none());
        assert!(export_contract("bcryptjs", "hashPassword").is_none());
        assert!(export_contract("zod", "safeParse").is_none());
        assert!(method_contract("zod", "safeParse").is_some());
        assert!(method_contract("yup", "safeParse").is_none());
        assert!(is_react_cache("react", "cache"));
        assert!(!is_react_cache("next/cache", "cache"));
        assert!(!is_react_cache("react", "useMemo"));
    }
}
