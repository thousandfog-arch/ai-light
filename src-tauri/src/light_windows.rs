use ai_light::config::get_config_dir;
use ai_light::types::LightState;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::{Arc, Mutex};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, Position, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

const LIGHT_WINDOW_PREFIX: &str = "light-";
const SNAP_DISTANCE: i32 = 16;
const SNAP_GAP: i32 = 6;
const ALIGN_DISTANCE: i32 = 24;
const DETACH_NUDGE_DISTANCE: i32 = 32;
const WINDOW_PARKING_POSITION: i32 = -32_000;

#[derive(Debug, Clone)]
struct LightWindowEntry {
    project_id: String,
    label: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    group_id: Option<u64>,
    revealed: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct SavedPosition {
    x: i32,
    y: i32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedLayout {
    #[serde(default)]
    positions: HashMap<String, SavedPosition>,
    #[serde(default)]
    groups: HashMap<String, u64>,
}

#[derive(Debug)]
struct LightWindowState {
    entries: HashMap<String, LightWindowEntry>,
    saved_positions: HashMap<String, SavedPosition>,
    saved_groups: HashMap<String, u64>,
    pending_moves: HashSet<String>,
    next_group_id: u64,
    user_visible: bool,
}

impl Default for LightWindowState {
    fn default() -> Self {
        let layout = load_layout();
        let next_group_id = layout
            .groups
            .values()
            .copied()
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        Self {
            entries: HashMap::new(),
            saved_positions: layout.positions,
            saved_groups: layout.groups,
            pending_moves: HashSet::new(),
            next_group_id,
            user_visible: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct LightWindowManager {
    state: Mutex<LightWindowState>,
}

impl LightWindowManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sync(
        self: &Arc<Self>,
        app: &AppHandle,
        lights: &[LightState],
        light_width: u16,
        label_font_size: u16,
        fallback_x: i32,
        fallback_y: i32,
    ) -> Result<(), String> {
        let desired: HashSet<&str> = lights
            .iter()
            .map(|light| light.project_id.as_str())
            .collect();

        let removed_labels = {
            let mut state = self.state.lock().map_err(|error| error.to_string())?;
            let removed: Vec<String> = state
                .entries
                .values()
                .filter(|entry| !desired.contains(entry.project_id.as_str()))
                .map(|entry| entry.label.clone())
                .collect();
            state
                .entries
                .retain(|_, entry| desired.contains(entry.project_id.as_str()));
            normalize_groups(&mut state.entries);
            removed
        };

        for label in removed_labels {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.destroy();
            }
        }

        let (logical_width, logical_height) = light_dimensions(light_width, label_font_size);
        for (index, light) in lights.iter().enumerate() {
            let existing = self
                .state
                .lock()
                .map_err(|error| error.to_string())?
                .entries
                .values()
                .any(|entry| entry.project_id == light.project_id);
            if existing {
                continue;
            }

            let (label, saved_position, saved_group) = {
                let state = self.state.lock().map_err(|error| error.to_string())?;
                (
                    unique_window_label(&state.entries, &light.project_id),
                    state.saved_positions.get(&light.project_id).copied(),
                    state.saved_groups.get(&light.project_id).copied(),
                )
            };

            let x = saved_position
                .map(|position| position.x)
                .unwrap_or(fallback_x + index as i32 * (logical_width as i32 + SNAP_GAP));
            let y = saved_position.map(|position| position.y).unwrap_or(fallback_y);

            let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
                .title(light.project_label.clone())
                .inner_size(logical_width, logical_height)
                // WebView2 may commit its first native frame before all builder
                // attributes settle. Create away from every normal desktop and
                // move on-screen only after the page has rendered and resized.
                .position(
                    f64::from(WINDOW_PARKING_POSITION),
                    f64::from(WINDOW_PARKING_POSITION),
                )
                .resizable(false)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .always_on_top(false)
                .focused(false)
                .skip_taskbar(true)
                .visible(false)
                .build()
                .map_err(|error| error.to_string())?;

            let outer_size = window.outer_size().map_err(|error| error.to_string())?;
            {
                let mut state = self.state.lock().map_err(|error| error.to_string())?;
                state.entries.insert(
                    label.clone(),
                    LightWindowEntry {
                        project_id: light.project_id.clone(),
                        label: label.clone(),
                        x,
                        y,
                        width: outer_size.width as i32,
                        height: outer_size.height as i32,
                        group_id: saved_group,
                        revealed: false,
                    },
                );
            }

            let weak_manager = Arc::downgrade(self);
            let event_app = app.clone();
            let event_label = label.clone();
            window.on_window_event(move |event| {
                let Some(manager) = weak_manager.upgrade() else {
                    return;
                };
                match event {
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        manager.hide_all(&event_app);
                    }
                    WindowEvent::Moved(position) => {
                        manager.handle_moved(&event_app, &event_label, position.x, position.y);
                    }
                    WindowEvent::Resized(size) => {
                        manager.handle_resized(
                            &event_app,
                            &event_label,
                            size.width as i32,
                            size.height as i32,
                        );
                    }
                    _ => {}
                }
            });
        }

        self.reconnect_touching_windows(app);

        // Never reveal the legacy main WebView. It used to flash in the centre
        // and steal focus while the light list transitioned through empty.
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.hide();
        }

        app.emit("state-changed", lights.to_vec())
            .map_err(|error| error.to_string())
    }

    pub fn project_for_window(&self, label: &str) -> Option<String> {
        self.state
            .lock()
            .ok()?
            .entries
            .get(label)
            .map(|entry| entry.project_id.clone())
    }

    pub fn detach(&self, label: &str) -> bool {
        let mut changed = false;
        if let Ok(mut state) = self.state.lock() {
            let project_id = state.entries.get_mut(label).and_then(|entry| {
                let changed = entry.group_id.take().is_some();
                changed.then(|| entry.project_id.clone())
            });
            if let Some(project_id) = project_id {
                state.saved_groups.remove(&project_id);
                changed = true;
            }
            normalize_groups(&mut state.entries);
        }
        if changed {
            self.save_positions();
        }
        changed
    }

    pub fn detach_with_nudge(&self, app: &AppHandle, label: &str) -> bool {
        let target = {
            let Ok(mut state) = self.state.lock() else {
                return false;
            };
            let Some((x, y)) = detach_nudge_target(&state.entries, label) else {
                return false;
            };
            let Some(entry) = state.entries.get_mut(label) else {
                return false;
            };
            entry.group_id = None;
            entry.x = x;
            entry.y = y;
            let project_id = entry.project_id.clone();
            normalize_groups(&mut state.entries);
            state.pending_moves.insert(label.to_string());
            state
                .saved_positions
                .insert(project_id.clone(), SavedPosition { x, y });
            state.saved_groups.remove(&project_id);
            (x, y)
        };

        if let Some(window) = app.get_webview_window(label) {
            let _ = window.set_position(Position::Physical(PhysicalPosition::new(
                target.0, target.1,
            )));
        }
        self.save_positions();
        true
    }

    pub fn is_attached(&self, label: &str) -> bool {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.entries.get(label).map(|entry| entry.group_id.is_some()))
            .unwrap_or(false)
    }

    pub fn prepare_bottom_anchored_resize(
        &self,
        label: &str,
        width: i32,
        height: i32,
    ) -> Option<(i32, i32)> {
        let position = {
            let mut state = self.state.lock().ok()?;
            let (project_id, x, y) = {
                let entry = state.entries.get_mut(label)?;
                let bottom = entry.y + entry.height;
                entry.width = width;
                entry.height = height;
                entry.y = bottom - height;
                (entry.project_id.clone(), entry.x, entry.y)
            };
            state.pending_moves.insert(label.to_string());
            state
                .saved_positions
                .insert(project_id, SavedPosition { x, y });
            (x, y)
        };
        self.save_positions();
        Some(position)
    }

    pub fn hide_all(&self, app: &AppHandle) {
        if let Ok(mut state) = self.state.lock() {
            state.user_visible = false;
        }
        for window in app.webview_windows().values() {
            if window.label() == "main" || window.label().starts_with(LIGHT_WINDOW_PREFIX) {
                let _ = window.hide();
            }
        }
    }

    pub fn toggle_all(&self, app: &AppHandle) -> Result<(), String> {
        let show = {
            let mut state = self.state.lock().map_err(|error| error.to_string())?;
            state.user_visible = !state.user_visible;
            state.user_visible
        };

        for window in app.webview_windows().values() {
            if !window.label().starts_with(LIGHT_WINDOW_PREFIX) {
                continue;
            }
            if show {
                let _ = window.show();
            } else {
                let _ = window.hide();
            }
        }
        Ok(())
    }

    pub fn show_when_ready(&self, window: &WebviewWindow) -> Result<bool, String> {
        if !window.label().starts_with(LIGHT_WINDOW_PREFIX) {
            return Ok(false);
        }
        let (x, y, should_show) = {
            let state = self.state.lock().map_err(|error| error.to_string())?;
            let entry = state
                .entries
                .get(window.label())
                .ok_or_else(|| "Light window state is not available.".to_string())?;
            (entry.x, entry.y, state.user_visible)
        };
        let position = crate::window_state::visible_window_position(
            window,
            PhysicalPosition::new(x, y),
        )?;
        if let Ok(mut state) = self.state.lock() {
            state.pending_moves.insert(window.label().to_string());
        }
        window
            .set_position(Position::Physical(position))
            .map_err(|error| error.to_string())?;
        window
            .set_always_on_top(true)
            .map_err(|error| error.to_string())?;
        {
            let mut state = self.state.lock().map_err(|error| error.to_string())?;
            let project_id = state.entries.get_mut(window.label()).map(|entry| {
                entry.x = position.x;
                entry.y = position.y;
                entry.revealed = true;
                entry.project_id.clone()
            });
            if let Some(project_id) = project_id {
                state.saved_positions.insert(
                    project_id,
                    SavedPosition {
                        x: position.x,
                        y: position.y,
                    },
                );
            }
        }
        self.save_positions();
        if should_show {
            window.show().map_err(|error| error.to_string())?;
        }
        Ok(should_show)
    }

    fn reconnect_touching_windows(&self, app: &AppHandle) {
        let mut native_moves = Vec::new();
        let mut groups_changed = false;
        if let Ok(mut state) = self.state.lock() {
            connect_touching_entries(&mut state);
            native_moves = align_all_groups(&mut state);
            groups_changed = capture_current_groups(&mut state);
        }
        self.apply_native_moves(app, native_moves);
        if groups_changed {
            self.save_positions();
        }
    }

    fn handle_moved(&self, app: &AppHandle, label: &str, x: i32, y: i32) {
        let mut native_moves = Vec::new();
        let mut should_save = false;

        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };

            if state.pending_moves.remove(label) {
                let project_id = if let Some(entry) = state.entries.get_mut(label) {
                    entry.x = x;
                    entry.y = y;
                    Some(entry.project_id.clone())
                } else {
                    None
                };
                if let Some(project_id) = project_id {
                    state
                        .saved_positions
                        .insert(project_id, SavedPosition { x, y });
                    should_save = true;
                }
            } else {
                let Some(moved_before) = state.entries.get(label).cloned() else {
                    return;
                };
                let dx = x - moved_before.x;
                let dy = y - moved_before.y;
                let group_id = moved_before.group_id;

                if let Some(entry) = state.entries.get_mut(label) {
                    entry.x = x;
                    entry.y = y;
                }

                if let Some(group_id) = group_id {
                    let companions: Vec<String> = state
                        .entries
                        .values()
                        .filter(|entry| entry.label != label && entry.group_id == Some(group_id))
                        .map(|entry| entry.label.clone())
                        .collect();
                    for companion_label in companions {
                        let target = if let Some(companion) = state.entries.get_mut(&companion_label) {
                            companion.x += dx;
                            companion.y += dy;
                            Some((companion.x, companion.y))
                        } else {
                            None
                        };
                        if let Some((companion_x, companion_y)) = target {
                            native_moves.push((companion_label.clone(), companion_x, companion_y));
                            state.pending_moves.insert(companion_label);
                        }
                    }
                    if let Some((neighbor_label, snap_x, snap_y)) =
                        nearest_snap(&state.entries, label)
                    {
                        let correction_x = snap_x - state.entries[label].x;
                        let correction_y = snap_y - state.entries[label].y;
                        let group_labels: Vec<String> = state
                            .entries
                            .values()
                            .filter(|entry| entry.group_id == Some(group_id))
                            .map(|entry| entry.label.clone())
                            .collect();
                        for group_label in group_labels {
                            if let Some(entry) = state.entries.get_mut(&group_label) {
                                entry.x += correction_x;
                                entry.y += correction_y;
                                native_moves.push((group_label.clone(), entry.x, entry.y));
                                state.pending_moves.insert(group_label);
                            }
                        }
                        connect_entries(&mut state, label, &neighbor_label);
                    }
                } else if let Some((neighbor_label, snap_x, snap_y)) =
                    nearest_snap(&state.entries, label)
                {
                    if let Some(entry) = state.entries.get_mut(label) {
                        entry.x = snap_x;
                        entry.y = snap_y;
                    }
                    connect_entries(&mut state, label, &neighbor_label);
                    state.pending_moves.insert(label.to_string());
                    native_moves.push((label.to_string(), snap_x, snap_y));
                }

                connect_touching_entries(&mut state);
                native_moves.extend(align_all_groups(&mut state));
                capture_current_groups(&mut state);

                let saved: Vec<(String, SavedPosition)> = state
                    .entries
                    .values()
                    .map(|entry| {
                        (
                            entry.project_id.clone(),
                            SavedPosition {
                                x: entry.x,
                                y: entry.y,
                            },
                        )
                    })
                    .collect();
                for (project_id, position) in saved {
                    state.saved_positions.insert(project_id, position);
                }
                should_save = true;
            }
        }

        self.apply_native_moves(app, native_moves);

        if should_save {
            self.save_positions();
        }
    }

    fn handle_resized(&self, app: &AppHandle, label: &str, width: i32, height: i32) {
        let mut native_moves = Vec::new();
        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            let group_id = if let Some(entry) = state.entries.get_mut(label) {
                entry.width = width;
                entry.height = height;
                entry.group_id
            } else {
                return;
            };

            let Some(group_id) = group_id else {
                return;
            };
            let mut group_labels: Vec<String> = state
                .entries
                .values()
                .filter(|entry| entry.group_id == Some(group_id))
                .map(|entry| entry.label.clone())
                .collect();
            group_labels.sort_by_key(|item| state.entries.get(item).map(|entry| entry.x));
            let Some(first) = group_labels
                .first()
                .and_then(|item| state.entries.get(item))
                .cloned()
            else {
                return;
            };
            let anchor_bottom = first.y + first.height;
            let mut next_x = first.x;
            for group_label in group_labels {
                let target = if let Some(entry) = state.entries.get_mut(&group_label) {
                    let aligned_y = anchor_bottom - entry.height;
                    let moved = entry.x != next_x || entry.y != aligned_y;
                    entry.x = next_x;
                    entry.y = aligned_y;
                    next_x += entry.width + SNAP_GAP;
                    moved.then_some((entry.x, entry.y))
                } else {
                    None
                };
                if let Some((x, y)) = target {
                    state.pending_moves.insert(group_label.clone());
                    native_moves.push((group_label, x, y));
                }
            }
        }

        self.apply_native_moves(app, native_moves);
        self.save_positions();
    }

    fn apply_native_moves(&self, app: &AppHandle, moves: Vec<(String, i32, i32)>) {
        let revealed = self
            .state
            .lock()
            .map(|state| {
                state
                    .entries
                    .values()
                    .filter(|entry| entry.revealed)
                    .map(|entry| entry.label.clone())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let mut final_moves = HashMap::new();
        for (label, x, y) in moves {
            final_moves.insert(label, (x, y));
        }
        for (label, (x, y)) in final_moves {
            if !revealed.contains(&label) {
                continue;
            }
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.set_position(Position::Physical(PhysicalPosition::new(x, y)));
            }
        }
    }

    fn save_positions(&self) {
        let layout = match self.state.lock() {
            Ok(state) => SavedLayout {
                positions: state.saved_positions.clone(),
                groups: state.saved_groups.clone(),
            },
            Err(_) => return,
        };
        let _ = save_layout(&layout);
    }
}

fn capture_current_groups(state: &mut LightWindowState) -> bool {
    let current = state
        .entries
        .values()
        .filter_map(|entry| {
            entry
                .group_id
                .map(|group_id| (entry.project_id.clone(), group_id))
        })
        .collect::<Vec<_>>();
    let mut changed = false;
    for (project_id, group_id) in current {
        if state.saved_groups.insert(project_id, group_id) != Some(group_id) {
            changed = true;
        }
    }
    changed
}

fn connect_touching_entries(state: &mut LightWindowState) {
    let labels: Vec<String> = state.entries.keys().cloned().collect();
    for (index, left_label) in labels.iter().enumerate() {
        for right_label in labels.iter().skip(index + 1) {
            let Some(left) = state.entries.get(left_label).cloned() else { continue; };
            let Some(right) = state.entries.get(right_label).cloned() else { continue; };
            if windows_touch(&left, &right) {
                connect_entries(state, left_label, right_label);
            }
        }
    }
}

fn align_all_groups(state: &mut LightWindowState) -> Vec<(String, i32, i32)> {
    let groups: HashSet<u64> = state.entries.values().filter_map(|entry| entry.group_id).collect();
    let mut moves = Vec::new();
    for group_id in groups {
        let mut labels: Vec<String> = state.entries.values()
            .filter(|entry| entry.group_id == Some(group_id))
            .map(|entry| entry.label.clone()).collect();
        if labels.len() < 2 { continue; }
        labels.sort_by(|a, b| state.entries[a].x.cmp(&state.entries[b].x).then(a.cmp(b)));
        let first = state.entries[&labels[0]].clone();
        let anchor_bottom = first.y + first.height;
        let mut next_x = first.x;
        for label in labels {
            if let Some(entry) = state.entries.get_mut(&label) {
                let y = anchor_bottom - entry.height;
                let changed = entry.x != next_x || entry.y != y;
                entry.x = next_x;
                entry.y = y;
                next_x += entry.width + SNAP_GAP;
                if changed {
                    state.pending_moves.insert(label.clone());
                    moves.push((label, entry.x, entry.y));
                }
            }
        }
    }
    moves
}

pub fn light_dimensions(width: u16, label_font_size: u16) -> (f64, f64) {
    let width = f64::from(width.clamp(44, 100));
    let label_height = f64::from(label_font_size.clamp(8, 24)) * 1.55 + 4.0;
    (width, (width * 2.08 + label_height).ceil())
}

fn unique_window_label(entries: &HashMap<String, LightWindowEntry>, project_id: &str) -> String {
    let hash = project_id
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |value, byte| {
            (value ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    let base = format!("{LIGHT_WINDOW_PREFIX}{hash:016x}");
    if !entries.contains_key(&base) {
        return base;
    }
    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if !entries.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn nearest_snap(
    entries: &HashMap<String, LightWindowEntry>,
    moved_label: &str,
) -> Option<(String, i32, i32)> {
    let moved = entries.get(moved_label)?;
    entries
        .values()
        .filter(|other| other.label != moved_label)
        .filter(|other| moved.group_id.is_none() || other.group_id != moved.group_id)
        .filter_map(|other| {
            if (moved.y - other.y).abs() > ALIGN_DISTANCE {
                return None;
            }
            let left_gap = (moved.x - (other.x + other.width + SNAP_GAP)).abs();
            let right_gap = ((moved.x + moved.width + SNAP_GAP) - other.x).abs();
            if left_gap <= SNAP_DISTANCE {
                Some((left_gap, other.label.clone(), other.x + other.width + SNAP_GAP, other.y))
            } else if right_gap <= SNAP_DISTANCE {
                Some((right_gap, other.label.clone(), other.x - moved.width - SNAP_GAP, other.y))
            } else {
                None
            }
        })
        .min_by_key(|candidate| candidate.0)
        .map(|(_, label, x, y)| (label, x, y))
}

fn detach_nudge_target(
    entries: &HashMap<String, LightWindowEntry>,
    label: &str,
) -> Option<(i32, i32)> {
    let clicked = entries.get(label)?;
    let group_id = clicked.group_id?;
    let mut group: Vec<&LightWindowEntry> = entries
        .values()
        .filter(|entry| entry.group_id == Some(group_id))
        .collect();
    group.sort_by(|left, right| left.x.cmp(&right.x).then(left.label.cmp(&right.label)));
    let index = group.iter().position(|entry| entry.label == label)?;

    if index == 0 {
        Some((clicked.x.saturating_sub(DETACH_NUDGE_DISTANCE), clicked.y))
    } else if index + 1 == group.len() {
        Some((clicked.x.saturating_add(DETACH_NUDGE_DISTANCE), clicked.y))
    } else {
        Some((clicked.x, clicked.y.saturating_sub(DETACH_NUDGE_DISTANCE)))
    }
}

fn windows_touch(left: &LightWindowEntry, right: &LightWindowEntry) -> bool {
    if (left.y - right.y).abs() > ALIGN_DISTANCE {
        return false;
    }
    ((left.x + left.width + SNAP_GAP) - right.x).abs() <= SNAP_DISTANCE
        || ((right.x + right.width + SNAP_GAP) - left.x).abs() <= SNAP_DISTANCE
}

fn connect_entries(state: &mut LightWindowState, left_label: &str, right_label: &str) {
    let left_group = state.entries.get(left_label).and_then(|entry| entry.group_id);
    let right_group = state.entries.get(right_label).and_then(|entry| entry.group_id);
    let group_id = match (left_group, right_group) {
        (Some(left), Some(right)) if left != right => {
            for entry in state.entries.values_mut() {
                if entry.group_id == Some(right) {
                    entry.group_id = Some(left);
                }
            }
            left
        }
        (Some(group), _) | (_, Some(group)) => group,
        (None, None) => {
            let group = state.next_group_id;
            state.next_group_id += 1;
            group
        }
    };
    if let Some(entry) = state.entries.get_mut(left_label) {
        entry.group_id = Some(group_id);
    }
    if let Some(entry) = state.entries.get_mut(right_label) {
        entry.group_id = Some(group_id);
    }
}

fn normalize_groups(entries: &mut HashMap<String, LightWindowEntry>) {
    let mut counts = HashMap::<u64, usize>::new();
    for group in entries.values().filter_map(|entry| entry.group_id) {
        *counts.entry(group).or_default() += 1;
    }
    for entry in entries.values_mut() {
        if entry
            .group_id
            .is_some_and(|group| counts.get(&group).copied().unwrap_or(0) < 2)
        {
            entry.group_id = None;
        }
    }
}

fn layout_path() -> std::path::PathBuf {
    get_config_dir().join("light-layout.json")
}

fn load_layout() -> SavedLayout {
    fs::read_to_string(layout_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_layout(layout: &SavedLayout) -> std::io::Result<()> {
    fs::create_dir_all(get_config_dir())?;
    let content = serde_json::to_string_pretty(layout).map_err(std::io::Error::other)?;
    fs::write(layout_path(), content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, x: i32, y: i32) -> LightWindowEntry {
        LightWindowEntry {
            project_id: label.to_string(),
            label: label.to_string(),
            x,
            y,
            width: 66,
            height: 144,
            group_id: None,
            revealed: true,
        }
    }

    #[test]
    fn finds_horizontal_snap_target() {
        let entries = HashMap::from([
            ("a".to_string(), entry("a", 100, 100)),
            ("b".to_string(), entry("b", 174, 108)),
        ]);
        let snap = nearest_snap(&entries, "b").unwrap();
        assert_eq!(snap, ("a".to_string(), 172, 100));
    }

    #[test]
    fn rejects_windows_that_are_not_aligned() {
        let entries = HashMap::from([
            ("a".to_string(), entry("a", 100, 100)),
            ("b".to_string(), entry("b", 174, 180)),
        ]);
        assert!(nearest_snap(&entries, "b").is_none());
    }

    #[test]
    fn nudges_outer_group_members_outward_and_middle_member_up() {
        let mut entries = HashMap::from([
            ("a".to_string(), entry("a", 100, 100)),
            ("b".to_string(), entry("b", 172, 100)),
            ("c".to_string(), entry("c", 244, 100)),
        ]);
        for item in entries.values_mut() {
            item.group_id = Some(7);
        }

        assert_eq!(detach_nudge_target(&entries, "a"), Some((68, 100)));
        assert_eq!(detach_nudge_target(&entries, "b"), Some((172, 68)));
        assert_eq!(detach_nudge_target(&entries, "c"), Some((276, 100)));
    }
}
