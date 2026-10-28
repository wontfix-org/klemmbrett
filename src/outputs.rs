//! Global output positions from `zxdg_output_v1`.
//!
//! `wl_output.geometry` is often `(0, 0)` for every output. The desktop layout
//! lives in this protocol instead, in the same space as a compositor pointer.

use std::collections::HashMap;

use uuid::Uuid;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};

/// Logical origin of each output, keyed the same way GPUI keys a Wayland display:
/// UUIDv5 of the output name in the DNS namespace.
pub fn logical_output_positions() -> HashMap<Uuid, (i32, i32)> {
    read_positions().unwrap_or_default()
}

pub(crate) fn output_uuid(name: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, name.as_bytes())
}

fn read_positions() -> Result<HashMap<Uuid, (i32, i32)>, String> {
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
    if let Some(manager) = state.manager.clone() {
        let outputs: Vec<_> = state
            .outputs
            .values()
            .map(|output| (output.proxy.clone(), output.proxy.id()))
            .collect();
        for (output, id) in outputs {
            state
                .xdg
                .push(manager.get_xdg_output(&output, &qh, id));
        }
    }
    queue
        .roundtrip(&mut state)
        .map_err(|err| format!("failed to read xdg output layout: {err}"))?;

    let mut positions = HashMap::new();
    for output in state.outputs.values() {
        if let (Some(name), Some(x), Some(y)) = (&output.name, output.x, output.y) {
            positions.insert(output_uuid(name), (x, y));
        }
    }
    Ok(positions)
}

#[derive(Default)]
struct State {
    manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    outputs: HashMap<wayland_client::backend::ObjectId, OutputSlot>,
    xdg: Vec<zxdg_output_v1::ZxdgOutputV1>,
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
                let output = registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
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
                state.manager = Some(registry.bind::<
                    zxdg_output_manager_v1::ZxdgOutputManagerV1,
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
}
