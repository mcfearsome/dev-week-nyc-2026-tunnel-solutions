//! `tnls-rendezvous` as a library: capability-scoped file sending over BitTorrent.
//!
//! The `tnls-rendezvous` binary (see `main.rs`) is the CLI/plugin front-end; this library
//! exposes the same building blocks so embedders (e.g. the egui GUI) can seed, fetch, and
//! drive the share/get flows in-process instead of shelling out.

pub mod bittorrent;
pub mod get;
pub mod server;
pub mod share;
