//! Data providers for panel widgets: metric catalog, hwmon, NVIDIA GPUs,
//! batteries, media, weather, applications, Home Assistant, notifications
//! and command output. Each talks to the system through files or small
//! helper programs, so the crate needs no D-Bus or HTTP library.

pub mod apps;
pub mod battery;
pub mod clock;
pub mod command;
pub mod focus;
pub mod hass;
mod http;
pub mod hwmon;
pub mod media;
pub mod metrics;
pub mod notify;
pub mod nvidia;
pub mod weather;
