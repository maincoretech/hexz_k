/// CLI benchmark runner (`hexz bench`).
pub mod bench;
/// egui-based GUI (Keka-style archive manager).
#[cfg(feature = "gui")]
pub mod gui;
/// Pack command: directory → .hxz archive.
pub mod pack;
/// Read, list, extract, and inspect commands.
pub mod read;
