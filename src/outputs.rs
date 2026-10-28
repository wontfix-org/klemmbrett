//! Global output positions from `zxdg_output_v1`.
//!
//! `wl_output.geometry` is often `(0, 0)` for every output. The desktop layout
//! lives in this protocol instead, in the same space as a compositor pointer.

use std::collections::HashMap;

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

pub fn output_layout() -> OutputLayout {
    read_layout().unwrap_or_default()
}

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
    let connection = Connection::connect_to_env()
        .map_err(|err| format!("failed to connect to Wayland: {err}"))?;
    let mut queue: EventQueue<State> = connection.new_event_queue();
    let qh = queue.handle();
    let _registry = connection.display().get_registry(&qh, ());
    let mut state = State::default();
    queue
        .roundtrip(&mut state)
        .map_err(|err| format!("failed to bind Wayland outputs: {err}"))?;
    queue
        .roundtrip(&mut state)
        .map_err(|err| format!("failed to read Wayland outputs: {err}"))?;
    if let Some(manager) = state.xdg_manager.clone() {
        let outputs: Vec<_> = state
            .outputs
            .values()
            .map(|output| (output.proxy.clone(), output.proxy.id()))
            .collect();
        for (output, id) in outputs {
            state.xdg.push(manager.get_xdg_output(&output, &qh, id));
        }
    }
    queue
        .roundtrip(&mut state)
        .map_err(|err| format!("failed to read xdg output layout: {err}"))?;
    if state.toplevel_manager.is_some() {
        queue
            .roundtrip(&mut state)
            .map_err(|err| format!("failed to read the focused toplevel: {err}"))?;
    }

    let mut positions = HashMap::new();
    for output in state.outputs.values() {
        if let (Some(name), Some(x), Some(y)) = (&output.name, output.x, output.y) {
            positions.insert(output_uuid(name), (x, y));
        }
    }
    Ok(OutputLayout {
        positions,
        focused: state.focused_output(),
    })
}

#[derive(Default)]
struct State {
    xdg_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    toplevel_manager: Option<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    outputs: HashMap<wayland_client::backend::ObjectId, OutputSlot>,
    xdg: Vec<zxdg_output_v1::ZxdgOutputV1>,
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
    proxy: wl_output::WlOutput,
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
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_output" => {
                let output =
                    registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                let id = output.id();
                state.outputs.insert(
                    id,
                    OutputSlot {
                        proxy: output,
                        name: None,
                        x: None,
                        y: None,
                    },
                );
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
            }
            "zwlr_foreign_toplevel_manager_v1" => {
                state.toplevel_manager = Some(registry.bind::<
                    zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
                    _,
                    _,
                >(name, version.min(3), qh, ()));
            }
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
