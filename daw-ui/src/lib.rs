//! Zoft UI - User interface layer (egui)

pub mod app;
pub mod timeline;
pub mod mixer;
pub mod piano_roll;
pub mod plugin_editor;
pub mod browser;
pub mod transport_bar;
pub mod track_list;
pub mod shortcuts;
pub mod theme;

pub use app::*;
pub use timeline::*;
pub use mixer::*;
pub use piano_roll::*;
pub use plugin_editor::*;
pub use browser::*;
pub use transport_bar::*;
pub use track_list::*;
pub use shortcuts::*;
pub use theme::*;