//! Length helpers resolved against the default font and a 1920×1080 monitor.

pub const EM_PX: f32 = 14.0;
pub const CH_PX: f32 = 9.0;
pub const MM_PX: f32 = 96.0 / 25.4;
pub const MONITOR_WIDTH: f32 = 1920.0;
pub const MONITOR_HEIGHT: f32 = 1080.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

pub fn axis_for_property(name: &str) -> Axis {
    match name {
        "height" | "max_height" | "font_size" => Axis::Vertical,
        _ => Axis::Horizontal,
    }
}
