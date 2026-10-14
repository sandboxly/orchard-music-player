//! Orchard Music Player: a themeable, Winamp-style music player drawn as a
//! character grid (Orchard Commander's `oc-grid`).

pub mod app;
pub mod audio;
pub mod browser;
pub mod input;
pub mod media;
pub mod meta;
pub mod playlist;
pub mod settings;
pub mod spectrum;
pub mod tasks;
pub mod theme;
pub mod ui;

pub use app::App;
