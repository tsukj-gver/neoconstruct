//! # construct
//!
//! A declarative, symmetric binary data parsing and building library.
//!
//! This is a Rust port of the Python [construct](https://github.com/construct/construct) library (v2.10.70).
//! The same definition can both parse binary data and build binary data.

pub mod binary;
pub mod combined;
pub mod constructs;
pub mod containers;
pub mod core;
pub mod expr;
pub mod gallery;
pub mod value;
