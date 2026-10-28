use klemmbrett::{BusInterface, Record, ShowRequest, HISTORY_BUS_NAME, OBJECT_PATH};
use std::collections::HashSet;
use std::sync::mpsc;

/// A request that must run on the GPUI thread.
#[derive(Debug)]
pub enum UiCmd {
    Show {
        widget: String,
        pointer: Option<(i32, i32)>,
        records: Option<Vec<Record>>,
        reply: Option<mpsc::Sender<Result<Vec<String>, String>>>,
    },
    RecordHistory(String),
    Quit,
}

/// Serves `Show` and `Prompt` and forwards valid requests to GPUI.
///
/// `Prompt` blocks in the bus method until `reply` receives the selection.
pub fn spawn(
    widget_names: HashSet<String>,
    on_demand: HashSet<String>,
    tx: async_channel::Sender<UiCmd>,
) {
    std::thread::Builder::new()
        .name("klemmbrett-dbus".into())
        .spawn(move || {
            let api = BusInterface::new(move |request: ShowRequest| {
                if let Err(err) = accept_show(
                    &request.widget,
                    &widget_names,
                    &on_demand,
                    request.records.is_some(),
                ) {
                    eprintln!("{err}");
                    return Err(err);
                }
                tx.send_blocking(UiCmd::Show {
                    widget: request.widget,
                    pointer: request.pointer,
                    records: request.records,
                    reply: request.reply,
                })
                .map_err(|err| format!("failed to request popup: {err}"))
            });
            match zbus::blocking::connection::Builder::session()
                .expect("session bus")
                .name(HISTORY_BUS_NAME)
                .expect("request bus name")
                .serve_at(OBJECT_PATH, api)
                .expect("export klemmbrett object")
                .build()
            {
                Ok(_conn) => std::thread::park(),
                Err(err) => eprintln!("Failed to claim {HISTORY_BUS_NAME} on D-Bus: {err}"),
            }
        })
        .expect("spawn D-Bus thread");
}

/// Rejects unknown widgets, and widgets with no provider when no records were passed.
fn accept_show(
    widget: &str,
    widget_names: &HashSet<String>,
    on_demand: &HashSet<String>,
    has_records: bool,
) -> Result<(), String> {
    if !widget_names.contains(widget) {
        return Err(if widget_names.is_empty() {
            format!("unknown widget {widget:?}: no popup widgets are configured")
        } else {
            let mut names: Vec<_> = widget_names.iter().cloned().collect();
            names.sort();
            format!("unknown widget {widget:?}; known: {}", names.join(", "))
        });
    }
    if !has_records && on_demand.contains(widget) {
        return Err(format!("widget {widget:?} has no provider; pass records"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> HashSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn on_demand_widgets_require_records() {
        let known = names(&["history", "pick"]);
        let on_demand = names(&["pick"]);
        assert!(accept_show("history", &known, &on_demand, false).is_ok());
        assert!(accept_show("pick", &known, &on_demand, true).is_ok());
        let error = accept_show("pick", &known, &on_demand, false).unwrap_err();
        assert!(error.contains("has no provider"), "{error}");
        assert!(error.contains("pass records"), "{error}");
    }

    #[test]
    fn unknown_widgets_are_rejected_before_the_provider_check() {
        let error =
            accept_show("missing", &names(&["history"]), &HashSet::new(), false).unwrap_err();
        assert!(error.contains("unknown widget"), "{error}");
        assert!(error.contains("history"), "{error}");
        let error = accept_show("missing", &HashSet::new(), &HashSet::new(), true).unwrap_err();
        assert!(error.contains("no popup widgets"), "{error}");
    }
}
