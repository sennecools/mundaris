//! Mathematical conventions and future reference-frame primitives for Mundaris.
//!
//! Authoritative astronomical and simulation calculations are expected to use
//! `f64` where precision matters. GPU-facing and observer-local render data should
//! use `f32`, with conversions performed deliberately at the rendering boundary.
//! Full coordinate and reference-frame APIs are intentionally not defined yet.

#![forbid(unsafe_code)]
