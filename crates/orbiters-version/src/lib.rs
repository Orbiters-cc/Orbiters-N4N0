//! The running program's version: what people see, what workers and the shell compare, what goes in user agents.
//!
//! It is not compiled into the libraries. Each program (orbiters-desktop, its workers, the setup) reads it from
//! `ORBITERS_VERSION`, which cargo sets from `.cargo/config.toml` (`[env]`, written by `cargo xtask bump`), and hands it
//! here first thing in `main`. A development bump then changes that one value: cargo recompiles the programs that read
//! it and none of the libraries, which before recompiled every workspace crate at each of the day's bumps (`env!
//! ("CARGO_PKG_VERSION")` changed in all of them). Never read `ORBITERS_VERSION` from a library: it would make that
//! library, and everything above it, recompile at every bump again.

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// Gives the program's version, once, before anything reads it: `orbiters_version::set(env!("ORBITERS_VERSION"))`.
/// A second call with another value is a bug (two versions in one process) and panics.
pub fn set(version: &'static str) {
    let set = *VERSION.get_or_init(|| version);
    assert_eq!(set, version, "the program's version was given twice, differently");
}

/// The running program's version. In a library's own tests, where no program gave one, the workspace's crate version.
pub fn get() -> &'static str {
    VERSION.get().copied().unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_programs_version_is_the_one_it_gave() {
        assert_eq!(super::get(), env!("CARGO_PKG_VERSION"));
        super::set("9.8.7-dev.6");
        super::set("9.8.7-dev.6");
        assert_eq!(super::get(), "9.8.7-dev.6");
        assert!(std::panic::catch_unwind(|| super::set("1.0.0")).is_err());
    }
}
