//! Core domain: pure data types shared across every layer.
//! No I/O, no terminal, no audio backend — just the music model and the
//! player's logical state.

pub mod model;
pub mod player;
pub mod shuffle;
