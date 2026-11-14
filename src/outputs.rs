//! Global output positions from `zxdg_output_v1`.
//!
//! `wl_output.geometry` is often `(0, 0)` for every output. The desktop layout
//! lives in this protocol instead, in the same space as a compositor pointer.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::journal;
use uuid::Uuid;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};

/// Output origins plus the output of the keyboard-focused toplevel, when the
/// compositor provides `zwlr_foreign_toplevel_manager_v1`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutputLayout {
    /// Logical origin of each output, keyed the same way GPUI keys a Wayland display.
    pub positions: HashMap<Uuid, (i32, i32)>,
    /// UUIDv5 of the focused toplevel's output name. `None` when it cannot be identified.
    pub focused: Option<Uuid>,
}

/// Logical origin of each output, keyed the same way GPUI keys a Wayland display:
/// UUIDv5 of the output name in the DNS namespace.
pub fn logical_output_positions() -> HashMap<Uuid, (i32, i32)> {
    output_layout().positions
}

/// The live layout when `start` subscribed, otherwise one fresh read.
pub fn output_layout() -> OutputLayout {
    if let Some(layout) = TRACKED.lock().ok().and_then(|slot| slot.clone()) {
        return layout;
    }
    read_layout().unwrap_or_default()
}

/// Subscribes to `zxdg_output_v1` unless `requested` is `Some(false)`.
///
/// A missing protocol logs a warning when tracking was left unset, and aborts
/// startup when it was set to `true`. With no subscription, popup placement
/// reads the layout once per popup.
pub fn start(requested: Option<bool>) -> Result<(), String> {
    if requested == Some(false) {
        return Ok(());
    }
    match open() {
        Ok((connection, queue, state)) if state.xdg_manager.is_some() => {
            watch(connection, queue, state)
        }
        Ok(_) => crate::track::finish(
            requested,
            "output layout tracking",
            Err("zxdg_output_v1 is not available".into()),
        ),
        Err(err) => crate::track::finish(requested, "output layout tracking", Err(err)),
    }
}

static TRACKED: Mutex<Option<OutputLayout>> = Mutex::new(None);

/// Index of `focused` in `display_ids`. An unknown or missing focus uses `fallback`.
pub fn focused_display_index(
    display_ids: &[Option<Uuid>],
    focused: Option<Uuid>,
    fallback: usize,
) -> usize {
    focused
        .and_then(|id| {
            display_ids
                .iter()
                .position(|candidate| *candidate == Some(id))
        })
        .unwrap_or(fallback)
}

pub(crate) fn output_uuid(name: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, name.as_bytes())
}

fn read_layout() -> Result<OutputLayout, String> {
    let (_connection, _queue, state) = open()?;
    Ok(state.layout())
}

fn open() -> Result<(Connection, EventQueue<State>, State), String> {
    let connection = Connection::connect_to_env()
        .map_err(|err| format!("failed to connect to Wayland: {err}"))?;
    let mut queue: EventQueue<State> = connection.new_event_queue();
    let registry = connection.display().get_registry(&queue.handle(), ());
    let mut state = State::new(registry);
    queue
        .roundtrip(&mut state)
        .map_err(|err| format!("failed to bind Wayland outputs: {err}"))?;
    queue
        .roundtrip(&mut state)
        .map_err(|err| format!("failed to read Wayland outputs: {err}"))?;
    state.bind_xdg(&queue.handle());
    queue
        .roundtrip(&mut state)
        .map_err(|err| format!("failed to read xdg output layout: {err}"))?;
    if state.toplevel_manager.is_some() {
        queue
            .roundtrip(&mut state)
            .map_err(|err| format!("failed to read the focused toplevel: {err}"))?;
    }
    Ok((connection, queue, state))
}

fn watch(connection: Connection, queue: EventQueue<State>, state: State) -> Result<(), String> {
    let layout = state.layout();
    journal::info(&state.describe());
    publish(&layout);
    std::thread::Builder::new()
        .name("klemmbrett-outputs".into())
        .spawn(move || {
            let _connection = connection;
            follow(queue, state, layout);
        })
        .map_err(|err| {
            clear_tracked();
            format!("failed to watch the output layout: {err}")
        })?;
    Ok(())
}

fn follow(mut queue: EventQueue<State>, mut state: State, mut last: OutputLayout) {
    loop {
        if let Err(err) = queue.blocking_dispatch(&mut state) {
            journal::warn(&format!("output layout subscription ended: {err}"));
            clear_tracked();
            break;
        }
        let layout = state.layout();
        if layout != last {
            journal::info(&state.describe());
            publish(&layout);
            last = layout;
        }
    }
}

fn publish(layout: &OutputLayout) {
    if let Ok(mut slot) = TRACKED.lock() {
        *slot = Some(layout.clone());
    }
}

fn clear_tracked() {
    if let Ok(mut slot) = TRACKED.lock() {
        *slot = None;
    }
}

struct State {
    /// Kept alive so the registry proxy is not destroyed while we listen.
    #[allow(dead_code)]
    registry: wl_registry::WlRegistry,
    xdg_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    toplevel_manager: Option<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    outputs: HashMap<wayland_client::backend::ObjectId, OutputSlot>,
    toplevels: Vec<ToplevelSlot>,
}

struct ToplevelSlot {
    handle: zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    /// Outputs applied at the last `done`.
    outputs: Vec<wayland_client::backend::ObjectId>,
    /// Incremental enter/leave set, committed on `done`.
    pending_outputs: Vec<wayland_client::backend::ObjectId>,
    activated: bool,
    pending_activated: bool,
    saw_state: bool,
}

struct OutputSlot {
    global_name: u32,
    proxy: wl_output::WlOutput,
    xdg: Option<zxdg_output_v1::ZxdgOutputV1>,
    name: Option<String>,
    x: Option<i32>,
    y: Option<i32>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_output" => {
                    let output =
                        registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                    let id = output.id();
                    state.outputs.insert(
                        id,
                        OutputSlot {
                            global_name: name,
                            proxy: output,
                            xdg: None,
                            name: None,
                            x: None,
                            y: None,
                        },
                    );
                    state.bind_xdg(qh);
                }
                "zxdg_output_manager_v1" => {
                    state.xdg_manager = Some(
                        registry.bind::<zxdg_output_manager_v1::ZxdgOutputManagerV1, _, _>(
                            name,
                            version.min(3),
                            qh,
                            (),
                        ),
                    );
                    state.bind_xdg(qh);
                }
                "zwlr_foreign_toplevel_manager_v1" => {
                    state.toplevel_manager = Some(registry.bind::<
                        zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
                        _,
                        _,
                    >(name, version.min(3), qh, ()));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => state.remove_global(name),
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(slot) = state.outputs.get_mut(&output.id()) else {
            return;
        };
        if let wl_output::Event::Name { name } = event {
            slot.name = Some(name);
        }
    }
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        _: zxdg_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, wayland_client::backend::ObjectId> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        output_id: &wayland_client::backend::ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(slot) = state.outputs.get_mut(output_id) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                slot.x = Some(x);
                slot.y = Some(y);
            }
            zxdg_output_v1::Event::Name { name } if slot.name.is_none() => {
                slot.name = Some(name);
            }
            _ => {}
        }
    }
}

impl State {
    fn new(registry: wl_registry::WlRegistry) -> Self {
        Self {
            registry,
            xdg_manager: None,
            toplevel_manager: None,
            outputs: HashMap::new(),
            toplevels: Vec::new(),
        }
    }

    fn bind_xdg(&mut self, qh: &QueueHandle<Self>) {
        let Some(manager) = self.xdg_manager.clone() else {
            return;
        };
        let missing: Vec<_> = self
            .outputs
            .iter()
            .filter(|(_, output)| output.xdg.is_none())
            .map(|(id, output)| (id.clone(), output.proxy.clone()))
            .collect();
        for (id, output) in missing {
            let xdg = manager.get_xdg_output(&output, qh, id.clone());
            if let Some(slot) = self.outputs.get_mut(&id) {
                slot.xdg = Some(xdg);
            }
        }
    }

    fn remove_global(&mut self, name: u32) {
        let Some(id) = self
            .outputs
            .iter()
            .find(|(_, output)| output.global_name == name)
            .map(|(id, _)| id.clone())
        else {
            return;
        };
        let Some(slot) = self.outputs.remove(&id) else {
            return;
        };
        if let Some(xdg) = slot.xdg {
            xdg.destroy();
        }
        if slot.proxy.version() >= 3 {
            slot.proxy.release();
        }
    }

    fn layout(&self) -> OutputLayout {
        let mut positions = HashMap::new();
        for output in self.outputs.values() {
            if let (Some(name), Some(x), Some(y)) = (&output.name, output.x, output.y) {
                positions.insert(output_uuid(name), (x, y));
            }
        }
        OutputLayout {
            positions,
            focused: self.focused_output(),
        }
    }

    fn describe(&self) -> String {
        let mut parts: Vec<String> = self
            .outputs
            .values()
            .filter_map(|output| {
                Some(format!(
                    "{} at {},{}",
                    output.name.as_ref()?,
                    output.x?,
                    output.y?
                ))
            })
            .collect();
        parts.sort();
        let list = if parts.is_empty() {
            "none".to_owned()
        } else {
            parts.join(", ")
        };
        match self.focused_name() {
            Some(name) => format!("outputs: {list}; focused {name}"),
            None => format!("outputs: {list}"),
        }
    }

    fn focused_name(&self) -> Option<String> {
        let focused = self.focused_output()?;
        self.outputs.values().find_map(|output| {
            let name = output.name.as_ref()?;
            (output_uuid(name) == focused).then(|| name.clone())
        })
    }

    /// First activated toplevel, and the first of its outputs that has a name.
    fn focused_output(&self) -> Option<Uuid> {
        for toplevel in &self.toplevels {
            if !toplevel.activated {
                continue;
            }
            for output_id in &toplevel.outputs {
                let name = self
                    .outputs
                    .get(output_id)
                    .and_then(|output| output.name.as_deref());
                if let Some(name) = name {
                    return Some(output_uuid(name));
                }
            }
        }
        None
    }

    fn toplevel_mut(
        &mut self,
        handle: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    ) -> Option<&mut ToplevelSlot> {
        let id = handle.id();
        self.toplevels
            .iter_mut()
            .find(|toplevel| toplevel.handle.id() == id)
    }
}

impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.push(ToplevelSlot {
                handle: toplevel,
                outputs: Vec::new(),
                pending_outputs: Vec::new(),
                activated: false,
                pending_activated: false,
                saw_state: false,
            });
        }
    }

    event_created_child!(State, zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (
            zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
            ()
        ),
    ]);
}

impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, zwlr_foreign_toplevel_handle_v1::Event::Closed) {
            let id = handle.id();
            if let Some(index) = state
                .toplevels
                .iter()
                .position(|toplevel| toplevel.handle.id() == id)
            {
                let toplevel = state.toplevels.remove(index);
                toplevel.handle.destroy();
            }
            return;
        }
        let Some(toplevel) = state.toplevel_mut(handle) else {
            return;
        };
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::OutputEnter { output } => {
                let id = output.id();
                if !toplevel.pending_outputs.contains(&id) {
                    toplevel.pending_outputs.push(id);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputLeave { output } => {
                let id = output.id();
                toplevel
                    .pending_outputs
                    .retain(|output_id| *output_id != id);
            }
            zwlr_foreign_toplevel_handle_v1::Event::State { state: flags } => {
                toplevel.pending_activated = state_is_activated(&flags);
                toplevel.saw_state = true;
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => {
                toplevel.outputs.clone_from(&toplevel.pending_outputs);
                if toplevel.saw_state {
                    toplevel.activated = toplevel.pending_activated;
                }
                toplevel.saw_state = false;
            }
            _ => {}
        }
    }
}

fn state_is_activated(state: &[u8]) -> bool {
    let activated = zwlr_foreign_toplevel_handle_v1::State::Activated as u32;
    state
        .chunks_exact(4)
        .any(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) == activated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_uuid_matches_the_gpui_wayland_display_key() {
        let name = "DP-1";
        assert_eq!(
            output_uuid(name),
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, name.as_bytes())
        );
        assert_ne!(output_uuid("DP-1"), output_uuid("DP-2"));
    }

    #[test]
    fn focused_display_index_prefers_a_matching_output() {
        let first = output_uuid("DP-1");
        let second = output_uuid("DP-2");
        let ids = [Some(first), Some(second)];
        assert_eq!(focused_display_index(&ids, Some(second), 0), 1);
        assert_eq!(focused_display_index(&ids, None, 0), 0);
        assert_eq!(
            focused_display_index(&ids, Some(output_uuid("HDMI-1")), 0),
            0
        );
    }
}
