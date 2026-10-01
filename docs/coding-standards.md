# Coding standards

- Prefer clear, explicit Rust and domain-meaningful names. Use Rust naming conventions; conventional short math/index names are fine in tight scopes.
- Make units explicit in names or types (`radius_meters`, `elapsed_seconds`, radians). Never silently mix meters/kilometers or degrees/radians; introduce unit types only when a real repeated safety need exists.
- Choose `f32` or `f64` deliberately. Precision is architectural: simulation may need `f64`, while local rendering generally uses `f32`.
- Return `Result` for recoverable failures. Add context at application/composition boundaries; do not silence failures or log and re-return the same error without reason. Avoid `unwrap`/`expect` in production paths.
- Let ownership express architecture. Avoid global mutable state and unnecessary `Arc<Mutex<_>>`, `Rc<RefCell<_>>`, or cloning; synchronize only when actual concurrency requires it.
- Keep public APIs intentionally small. Prefer private implementation details until another module or crate has a real caller; split modules by responsibility when they grow.
- Project crates forbid unsafe code. Any future exception requires a narrow boundary, a documented safety argument, tests, and a measured justification.
- Document public crate purpose, important types, invariants, and non-obvious constraints. Comments should explain why or clarify a constraint, not paraphrase code.
- Use the standard library where sufficient. Add mature, focused dependencies only for concrete needs; avoid speculative frameworks, duplicate libraries, and unused dependencies.
- Do not add broad abstraction layers, generic infrastructure, or domain placeholder types for systems that have no implementation or caller yet.

Run `cargo fmt --all` before submitting changes. CI runs formatting, Clippy with warnings denied, and headless workspace tests.
