//! Live Wayland toplevel list via `zwlr_foreign_toplevel_manager_v1`.

use crate::Record;
use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{
    event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};

const DISPATCH_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub struct WindowList {
    snapshot: Arc<Mutex<Result<Vec<Record>, String>>>,
    activate: mpsc::Sender<String>,
}

impl WindowList {
    pub fn start() -> Self {
        let snapshot = Arc::new(Mutex::new(Err(
            "wayland window list is still connecting".into()
        )));
        let (activate_tx, activate_rx) = mpsc::channel();
        let thread_snapshot = snapshot.clone();
        std::thread::Builder::new()
            .name("klemmbrett-windows".into())
            .spawn(move || {
                if let Err(err) = run(thread_snapshot.clone(), activate_rx) {
                    if let Ok(mut slot) = thread_snapshot.lock() {
                        *slot = Err(err);
                    }
                }
            })
            .expect("spawn wayland window thread");
        Self {
            snapshot,
            activate: activate_tx,
        }
    }

    pub fn records(&self) -> Result<Vec<Record>, String> {
        self.snapshot
            .lock()
            .map_err(|_| "wayland window list lock was poisoned".to_string())?
            .clone()
    }

    pub fn activate(&self, key: &str) -> Result<(), String> {
        self.activate
            .send(key.to_owned())
            .map_err(|_| "wayland window thread is not running".to_string())
    }
}

struct Toplevel {
    handle: zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    key: String,
    title: String,
    app_id: String,
}

struct State {
    manager: Option<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    windows: HashMap<u32, Toplevel>,
    next_key: u64,
    snapshot: Arc<Mutex<Result<Vec<Record>, String>>>,
}

impl State {
    fn publish(&self) {
        let mut records: Vec<Record> = self
            .windows
            .values()
            .map(|window| window_record(&window.key, &window.title, &window.app_id))
            .collect();
        records.sort_by(|left, right| left.label.cmp(&right.label));
        if let Ok(mut slot) = self.snapshot.lock() {
            *slot = Ok(records);
        }
    }

    fn activate(&self, key: &str) {
        let Some(seat) = &self.seat else {
            eprintln!("klemmbrett: no wl_seat; cannot activate window {key}");
            return;
        };
        let Some(window) = self.windows.values().find(|window| window.key == key) else {
            eprintln!("klemmbrett: unknown window {key}");
            return;
        };
        window.handle.activate(seat);
    }
}

pub fn window_record(key: &str, title: &str, app_id: &str) -> Record {
    let label = if !title.is_empty() {
        title
    } else if !app_id.is_empty() {
        app_id
    } else {
        "Untitled"
    };
    Record {
        key: key.to_owned(),
        label: label.to_owned(),
        value: key.to_owned(),
    }
}

fn run(
    snapshot: Arc<Mutex<Result<Vec<Record>, String>>>,
    activate: mpsc::Receiver<String>,
) -> Result<(), String> {
    let connection = Connection::connect_to_env()
        .map_err(|err| format!("failed to connect to Wayland: {err}"))?;
    let mut queue: EventQueue<State> = connection.new_event_queue();
    let qh = queue.handle();
    let display = connection.display();
    let _registry = display.get_registry(&qh, ());
    let mut state = State {
        manager: None,
        seat: None,
        windows: HashMap::new(),
        next_key: 1,
        snapshot,
    };
    queue
        .roundtrip(&mut state)
        .map_err(|err| format!("failed to bind Wayland globals: {err}"))?;
    if state.manager.is_none() {
        return Err("zwlr_foreign_toplevel_manager_v1 is not available".into());
    }
    state.publish();

    loop {
        while let Ok(key) = activate.try_recv() {
            state.activate(&key);
        }
        if let Err(err) = connection.flush() {
            return Err(format!("failed to flush Wayland: {err}"));
        }
        match activate.recv_timeout(DISPATCH_TIMEOUT) {
            Ok(key) => state.activate(&key),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        queue
            .dispatch_pending(&mut state)
            .map_err(|err| format!("failed to dispatch Wayland events: {err}"))?;
        if let Some(guard) = connection.prepare_read() {
            let _ = guard.read();
        }
        queue
            .dispatch_pending(&mut state)
            .map_err(|err| format!("failed to dispatch Wayland events: {err}"))?;
    }
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
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "zwlr_foreign_toplevel_manager_v1" => {
                    state.manager = Some(registry.bind::<
                        zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
                        _,
                        _,
                    >(name, version.min(3), qh, ()));
                }
                "wl_seat" if state.seat.is_none() => {
                    state.seat =
                        Some(registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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
            let id = toplevel.id().protocol_id();
            let key = state.next_key.to_string();
            state.next_key += 1;
            state.windows.insert(
                id,
                Toplevel {
                    handle: toplevel,
                    key,
                    title: String::new(),
                    app_id: String::new(),
                },
            );
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
        let id = handle.id().protocol_id();
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                if let Some(window) = state.windows.get_mut(&id) {
                    window.title = title;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                if let Some(window) = state.windows.get_mut(&id) {
                    window.app_id = app_id;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => state.publish(),
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                if let Some(window) = state.windows.remove(&id) {
                    window.handle.destroy();
                    state.publish();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_label_prefers_title_then_app_id() {
        assert_eq!(window_record("1", "Inbox", "thunderbird").label, "Inbox");
        assert_eq!(window_record("2", "", "firefox").label, "firefox");
        assert_eq!(window_record("3", "", "").label, "Untitled");
        assert_eq!(window_record("4", "Chat", "").value, "4");
    }
}
