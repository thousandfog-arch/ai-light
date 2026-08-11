use ai_light::config::{load_app_config, save_app_config, AppConfig};
use tauri::{PhysicalPosition, Position, WebviewWindow};

const FALLBACK_OFFSET: i32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisplayBounds {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowSize {
    width: i32,
    height: i32,
}

pub fn restore_main_window_position(
    window: &WebviewWindow,
    config: &AppConfig,
) -> Result<(), String> {
    let saved = PhysicalPosition::new(config.window_x, config.window_y);
    let target = visible_position(window, saved)?;

    window
        .set_position(Position::Physical(target))
        .map_err(|error| error.to_string())?;

    if target != saved {
        save_position(target.x, target.y)?;
    }

    Ok(())
}

pub fn ensure_window_visible(window: &WebviewWindow) -> Result<(), String> {
    let current = window.outer_position().map_err(|error| error.to_string())?;
    let target = visible_window_position(window, current)?;

    if target != current {
        window
            .set_position(Position::Physical(target))
            .map_err(|error| error.to_string())?;
        save_position(target.x, target.y)?;
    }

    Ok(())
}

pub fn visible_window_position(
    window: &WebviewWindow,
    requested: PhysicalPosition<i32>,
) -> Result<PhysicalPosition<i32>, String> {
    visible_position(window, requested)
}

pub fn save_position(x: i32, y: i32) -> Result<(), String> {
    let mut config = load_app_config();
    if config.window_x == x && config.window_y == y {
        return Ok(());
    }

    config.window_x = x;
    config.window_y = y;
    save_app_config(&config).map_err(|error| error.to_string())
}

fn visible_position(
    window: &WebviewWindow,
    requested: PhysicalPosition<i32>,
) -> Result<PhysicalPosition<i32>, String> {
    let size = window.outer_size().map_err(|error| error.to_string())?;
    let monitors = window
        .available_monitors()
        .map_err(|error| error.to_string())?
        .iter()
        .map(display_bounds)
        .collect::<Vec<_>>();
    let primary = window
        .primary_monitor()
        .map_err(|error| error.to_string())?
        .as_ref()
        .map(display_bounds);

    let target = clamp_window_position(
        requested,
        WindowSize {
            width: size.width.min(i32::MAX as u32) as i32,
            height: size.height.min(i32::MAX as u32) as i32,
        },
        &monitors,
        primary,
    );

    Ok(target)
}

fn display_bounds(monitor: &tauri::Monitor) -> DisplayBounds {
    let position = monitor.position();
    let size = monitor.size();
    DisplayBounds {
        x: position.x,
        y: position.y,
        width: size.width.min(i32::MAX as u32) as i32,
        height: size.height.min(i32::MAX as u32) as i32,
    }
}

fn clamp_window_position(
    requested: PhysicalPosition<i32>,
    window: WindowSize,
    monitors: &[DisplayBounds],
    primary: Option<DisplayBounds>,
) -> PhysicalPosition<i32> {
    if monitors.is_empty() {
        return requested;
    }

    let requested_bounds = DisplayBounds {
        x: requested.x,
        y: requested.y,
        width: window.width.max(1),
        height: window.height.max(1),
    };

    let intersecting = monitors
        .iter()
        .copied()
        .max_by_key(|monitor| intersection_area(requested_bounds, *monitor))
        .filter(|monitor| intersection_area(requested_bounds, *monitor) > 0);

    let (monitor, desired) = match intersecting {
        Some(monitor) => (monitor, requested),
        None => {
            let monitor = primary.unwrap_or(monitors[0]);
            (
                monitor,
                PhysicalPosition::new(
                    monitor.x.saturating_add(FALLBACK_OFFSET),
                    monitor.y.saturating_add(FALLBACK_OFFSET),
                ),
            )
        }
    };

    PhysicalPosition::new(
        clamp_axis(desired.x, window.width, monitor.x, monitor.width),
        clamp_axis(desired.y, window.height, monitor.y, monitor.height),
    )
}

fn clamp_axis(position: i32, window_size: i32, display_start: i32, display_size: i32) -> i32 {
    if window_size >= display_size {
        return display_start;
    }

    position.clamp(
        display_start,
        display_start.saturating_add(display_size - window_size),
    )
}

fn intersection_area(left: DisplayBounds, right: DisplayBounds) -> i64 {
    let overlap_width = (left.x.saturating_add(left.width))
        .min(right.x.saturating_add(right.width))
        .saturating_sub(left.x.max(right.x))
        .max(0);
    let overlap_height = (left.y.saturating_add(left.height))
        .min(right.y.saturating_add(right.height))
        .saturating_sub(left.y.max(right.y))
        .max(0);
    i64::from(overlap_width) * i64::from(overlap_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: DisplayBounds = DisplayBounds {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    const LEFT: DisplayBounds = DisplayBounds {
        x: -1280,
        y: 0,
        width: 1280,
        height: 1024,
    };
    const WINDOW: WindowSize = WindowSize {
        width: 300,
        height: 200,
    };

    #[test]
    fn keeps_a_visible_position() {
        let position = clamp_window_position(
            PhysicalPosition::new(250, 260),
            WINDOW,
            &[PRIMARY],
            Some(PRIMARY),
        );
        assert_eq!(position, PhysicalPosition::new(250, 260));
    }

    #[test]
    fn clamps_a_partially_offscreen_window() {
        let position = clamp_window_position(
            PhysicalPosition::new(1850, 1040),
            WINDOW,
            &[PRIMARY],
            Some(PRIMARY),
        );
        assert_eq!(position, PhysicalPosition::new(1620, 880));
    }

    #[test]
    fn restores_a_fully_offscreen_window_to_primary_monitor() {
        let position = clamp_window_position(
            PhysicalPosition::new(9000, 9000),
            WINDOW,
            &[LEFT, PRIMARY],
            Some(PRIMARY),
        );
        assert_eq!(position, PhysicalPosition::new(32, 32));
    }

    #[test]
    fn preserves_negative_coordinates_on_a_left_monitor() {
        let position = clamp_window_position(
            PhysicalPosition::new(-900, 300),
            WINDOW,
            &[LEFT, PRIMARY],
            Some(PRIMARY),
        );
        assert_eq!(position, PhysicalPosition::new(-900, 300));
    }

    #[test]
    fn anchors_an_oversized_window_to_the_monitor_origin() {
        let position = clamp_window_position(
            PhysicalPosition::new(100, 100),
            WindowSize {
                width: 2400,
                height: 1400,
            },
            &[PRIMARY],
            Some(PRIMARY),
        );
        assert_eq!(position, PhysicalPosition::new(0, 0));
    }
}
