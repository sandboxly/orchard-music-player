//! Orchard Music Player: a themeable, Winamp-style music player drawn as a
//! character grid (`orchard-grid`, shared with Orchard Commander).

pub mod app;
pub mod audio;
pub mod browser;
pub mod media;
pub mod meta;
pub mod playlist;
pub mod settings;
pub mod spectrum;
pub mod ui;

pub use app::App;
